//! Scripted scenario driver executing a full tic-tac-toe game flow.
//!
//! Drives the following scenario against an active lane and covenant:
//! 1. Config `Init` with genesis authentication.
//! 2. Player A and Player B deposits.
//! 3. `CreateGame` by player A (staked, round count).
//! 4. `JoinGame` by player B.
//! 5. Player A turn carrier with opening ply and pre-commits.
//! 6. Player B turn carrier triggering cascade, round win, and match settlement.
//! 7. Player A winner pot withdrawal.

use std::sync::Arc;

use kaspa_consensus_core::{
    config::params::Params,
    constants::TX_VERSION_TOCCATA,
    subnets::SubnetworkId,
    tx::{TransactionOutpoint, UtxoEntry},
};
use kaspa_hashes::Hash;
use kaspa_rpc_core::api::rpc::RpcApi;
use secp256k1::Keypair;
use vprog_tictactoe_guest::{
    program::{
        action::encode::{
            GENESIS_SIG_PTR_TAG, encode_create_game_action, encode_deposit_action,
            encode_init_action, encode_join_game_action, encode_turn_action,
            encode_withdraw_action, game_two_user_access, game_user_access, user_config_access,
        },
        resources::{
            game::Cell,
            id::{config_resource_id, derive_game_resource, derive_user_resource},
        },
    },
    runtime::{
        genesis::GENESIS_PUBKEY,
        lock::{LockEnum, SchnorrLockView},
    },
};
use vprogs_core_types::{AccessMetadata, ResourceId};
use vprogs_l1_wallet::Wallet;
use vprogs_zk_abi::withdrawal::StandardSpk;
use vprogs_zk_backend_risc0_app_kit::{
    Bip340Signer, CarrierTxArgs, DEFAULT_MAX_SUBMIT_ATTEMPTS, DEFAULT_SUBMIT_RETRY_DELAY,
    LanePayload, SchnorrSigPtrSigner, SignerKind, SignerSpec, TailBlock, covenant_deposit_output,
    signed_deposit_tx, signed_lane_action_tx, signer::Signer,
};

use crate::config::Config;

/// Execution report returned by [`run`] containing the accepted transaction IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioReport {
    /// Transaction ID of the config `Init` carrier.
    pub init_txid: Hash,
    /// Transaction ID of player A's `Deposit` carrier.
    pub deposit_a_txid: Hash,
    /// Transaction ID of player B's `Deposit` carrier.
    pub deposit_b_txid: Hash,
    /// Transaction ID of player A's `CreateGame` carrier.
    pub create_game_txid: Hash,
    /// Transaction ID of player B's `JoinGame` carrier.
    pub join_game_txid: Hash,
    /// Transaction ID of player A's `Turn` carrier (opening ply and pre-commits).
    pub turn_a_txid: Hash,
    /// Transaction ID of player B's `Turn` carrier (reply ply and pre-commit settling match).
    pub turn_b_txid: Hash,
    /// Transaction ID of player A's `Withdraw` carrier.
    pub withdraw_txid: Hash,
    /// Schnorr public key of player A; the withdraw destination and exit-leaf owner.
    pub player_a_pubkey: [u8; 32],
    /// Resource ID of the game under test.
    pub game_id: ResourceId,
    /// Resource ID of player A.
    pub player_a_user_id: ResourceId,
    /// Resource ID of player B.
    pub player_b_user_id: ResourceId,
}

/// Shared execution context for submitting scenario carrier transactions.
struct CarrierContext<'a, C: RpcApi + ?Sized> {
    /// Funded wallet managing UTXO selection and submission.
    wallet: &'a Wallet<'a, C>,
    /// Keypair authorizing carrier transaction fees.
    operator_keypair: Keypair,
    /// Target execution lane subnetwork identifier.
    subnetwork_id: SubnetworkId,
    /// Consensus parameters for mass calculation.
    params: &'a Params,
}

/// Builds the `Init` action payload for bootstrapping config state.
pub fn build_init_payload(
    covenant_id: &[u8; 32],
    min_withdrawal: u64,
    turn_ttl: u64,
    genesis_lock: &LockEnum<'_>,
) -> LanePayload {
    let action_body = encode_init_action(0, min_withdrawal, turn_ttl, covenant_id, genesis_lock);
    LanePayload::new()
        .access(AccessMetadata::write(config_resource_id()))
        .action(action_body)
        .signer(SignerSpec {
            resource_idx: 0,
            kind: SignerKind::SigPtr { tag: GENESIS_SIG_PTR_TAG },
            tail: TailBlock::Sig64,
        })
}

/// Builds a `Deposit` payload crediting an L1 deposit to a user.
pub fn build_deposit_payload(
    user_id: ResourceId,
    config_id: ResourceId,
    initial_lock: &LockEnum<'_>,
) -> LanePayload {
    let access = user_config_access(user_id, config_id);
    let action_body = encode_deposit_action(access.user_idx, access.config_idx, 0, initial_lock);
    LanePayload::new().access(access.access[0]).access(access.access[1]).action(action_body)
}

/// Builds a `CreateGame` payload opening a new staked game.
pub fn build_create_game_payload(
    creator_user_id: ResourceId,
    game_id: ResourceId,
    stake: u64,
    rounds: u8,
    mark: Cell,
) -> LanePayload {
    let access = game_user_access(game_id, creator_user_id);
    let action_body =
        encode_create_game_action(access.user_idx, access.game_idx, stake, rounds, mark);
    LanePayload::new().access(access.access[0]).access(access.access[1]).action(action_body).signer(
        SignerSpec {
            resource_idx: access.user_idx,
            kind: SignerKind::SigPtr { tag: SchnorrSigPtrSigner::TAG },
            tail: TailBlock::Sig64,
        },
    )
}

/// Builds a `JoinGame` payload joining an open game.
pub fn build_join_game_payload(joiner_user_id: ResourceId, game_id: ResourceId) -> LanePayload {
    let access = game_user_access(game_id, joiner_user_id);
    let action_body = encode_join_game_action(access.game_idx, access.user_idx);
    LanePayload::new().access(access.access[0]).access(access.access[1]).action(action_body).signer(
        SignerSpec {
            resource_idx: access.user_idx,
            kind: SignerKind::SigPtr { tag: SchnorrSigPtrSigner::TAG },
            tail: TailBlock::Sig64,
        },
    )
}

/// Builds player A's turn payload containing an opening move and pre-commits.
pub fn build_turn_a_payload(
    game_id: ResourceId,
    player_a_user_id: ResourceId,
    player_b_user_id: ResourceId,
) -> LanePayload {
    let access = game_two_user_access(game_id, player_a_user_id, player_b_user_id);
    let act1 = encode_turn_action(access.game_idx, access.first_user_idx, 0);
    let act2 = encode_turn_action(access.game_idx, access.first_user_idx, 1);
    let act3 = encode_turn_action(access.game_idx, access.first_user_idx, 2);
    LanePayload::new()
        .access(access.access[0])
        .access(access.access[1])
        .access(access.access[2])
        .action(act1)
        .action(act2)
        .action(act3)
        .signer(SignerSpec {
            resource_idx: access.first_user_idx,
            kind: SignerKind::SigPtr { tag: SchnorrSigPtrSigner::TAG },
            tail: TailBlock::Sig64,
        })
}

/// Builds player B's turn payload containing a reply move and pre-commit.
pub fn build_turn_b_payload(
    game_id: ResourceId,
    player_a_user_id: ResourceId,
    player_b_user_id: ResourceId,
) -> LanePayload {
    let access = game_two_user_access(game_id, player_a_user_id, player_b_user_id);
    let act1 = encode_turn_action(access.game_idx, access.second_user_idx, 3);
    let act2 = encode_turn_action(access.game_idx, access.second_user_idx, 4);
    LanePayload::new()
        .access(access.access[0])
        .access(access.access[1])
        .access(access.access[2])
        .action(act1)
        .action(act2)
        .signer(SignerSpec {
            resource_idx: access.second_user_idx,
            kind: SignerKind::SigPtr { tag: SchnorrSigPtrSigner::TAG },
            tail: TailBlock::Sig64,
        })
}

/// Builds a `Withdraw` payload for exiting player funds to L1.
pub fn build_withdraw_payload(
    user_id: ResourceId,
    config_id: ResourceId,
    amount: u64,
    dest: &StandardSpk<'_>,
) -> LanePayload {
    let access = user_config_access(user_id, config_id);
    let action_body = encode_withdraw_action(access.user_idx, access.config_idx, amount, dest);
    LanePayload::new().access(access.access[0]).access(access.access[1]).action(action_body).signer(
        SignerSpec {
            resource_idx: access.user_idx,
            kind: SignerKind::SigPtr { tag: SchnorrSigPtrSigner::TAG },
            tail: TailBlock::Sig64,
        },
    )
}

/// Executes the scripted tic-tac-toe game scenario.
pub async fn run<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    params: &Params,
    cfg: &Config,
) -> Result<ScenarioReport, String> {
    let operator_keypair = Keypair::from_secret_key(secp256k1::SECP256K1, &cfg.private_key);
    let wallet = Wallet::new(&**client, params, operator_keypair);
    let lane_subnet = SubnetworkId::from_namespace(cfg.lane_id.to_be_bytes());

    let ctx =
        CarrierContext { wallet: &wallet, operator_keypair, subnetwork_id: lane_subnet, params };

    let genesis_signer = Bip340Signer::from_secret_key(&cfg.genesis_key);
    let player_a = Bip340Signer::new();
    let player_b = Bip340Signer::new();

    let player_a_lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &player_a.pubkey() });
    let player_b_lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &player_b.pubkey() });

    let player_a_ilh = player_a_lock.id_hash();
    let player_b_ilh = player_b_lock.id_hash();

    let player_a_user_id = derive_user_resource(&player_a_ilh);
    let player_b_user_id = derive_user_resource(&player_b_ilh);
    let config_id = config_resource_id();
    let game_id = derive_game_resource(&player_a_ilh, 0);

    let covenant_id = cfg.covenant_id.as_bytes();

    // Step 1: Config Init.
    log::info!("issuing Step 1: Init config");
    let genesis_lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &GENESIS_PUBKEY });
    let init_payload = build_init_payload(&covenant_id, 1_000_000, cfg.turn_ttl, &genesis_lock);
    let init_txid = submit_action("init config", &ctx, &init_payload, |req| {
        genesis_signer.sign_digest(req.digest)
    })
    .await?;
    log::info!("Step 1 (Init) accepted: {init_txid}");
    tokio::time::sleep(cfg.step_delay).await;

    // Step 2: Deposit Player A.
    log::info!("issuing Step 2: Deposit Player A");
    let deposit_a_payload = build_deposit_payload(player_a_user_id, config_id, &player_a_lock);
    let deposit_a_txid = submit_deposit(
        "deposit player A",
        &ctx,
        &deposit_a_payload,
        &covenant_id,
        cfg.deposit_amount,
    )
    .await?;
    log::info!("Step 2 (Deposit Player A) accepted: {deposit_a_txid}");
    tokio::time::sleep(cfg.step_delay).await;

    // Step 3: Deposit Player B.
    log::info!("issuing Step 3: Deposit Player B");
    let deposit_b_payload = build_deposit_payload(player_b_user_id, config_id, &player_b_lock);
    let deposit_b_txid = submit_deposit(
        "deposit player B",
        &ctx,
        &deposit_b_payload,
        &covenant_id,
        cfg.deposit_amount,
    )
    .await?;
    log::info!("Step 3 (Deposit Player B) accepted: {deposit_b_txid}");
    tokio::time::sleep(cfg.step_delay).await;

    // Step 4: CreateGame.
    log::info!("issuing Step 4: CreateGame");
    let create_payload =
        build_create_game_payload(player_a_user_id, game_id, cfg.stake, cfg.rounds, Cell::X);
    let create_game_txid =
        submit_action("create game", &ctx, &create_payload, |req| player_a.sign_digest(req.digest))
            .await?;
    log::info!("Step 4 (CreateGame) accepted: {create_game_txid}");
    tokio::time::sleep(cfg.step_delay).await;

    // Step 5: JoinGame.
    log::info!("issuing Step 5: JoinGame");
    let join_payload = build_join_game_payload(player_b_user_id, game_id);
    let join_game_txid =
        submit_action("join game", &ctx, &join_payload, |req| player_b.sign_digest(req.digest))
            .await?;
    log::info!("Step 5 (JoinGame) accepted: {join_game_txid}");
    tokio::time::sleep(cfg.step_delay).await;

    // Step 6: Turn Player A (opening move + pre-commits).
    log::info!("issuing Step 6: Turn Player A (ply + precommits)");
    let turn_a_payload = build_turn_a_payload(game_id, player_a_user_id, player_b_user_id);
    let turn_a_txid = submit_action("turn player A", &ctx, &turn_a_payload, |req| {
        player_a.sign_digest(req.digest)
    })
    .await?;
    log::info!("Step 6 (Turn Player A) accepted: {turn_a_txid}");
    tokio::time::sleep(cfg.step_delay).await;

    // Step 7: Turn Player B (reply move + pre-commit triggering settlement).
    log::info!("issuing Step 7: Turn Player B (ply + precommit, settle match)");
    let turn_b_payload = build_turn_b_payload(game_id, player_a_user_id, player_b_user_id);
    let turn_b_txid = submit_action("turn player B", &ctx, &turn_b_payload, |req| {
        player_b.sign_digest(req.digest)
    })
    .await?;
    log::info!("Step 7 (Turn Player B) accepted: {turn_b_txid}");
    tokio::time::sleep(cfg.step_delay).await;

    // Step 8: Withdraw Winnings.
    log::info!("issuing Step 8: Withdraw winner pot");
    let dest_spk = StandardSpk::PubKey(&player_a.pubkey());
    let withdraw_amount = cfg.stake.checked_mul(2).unwrap_or(cfg.stake);
    let withdraw_payload =
        build_withdraw_payload(player_a_user_id, config_id, withdraw_amount, &dest_spk);
    let withdraw_txid = submit_action("withdraw winnings", &ctx, &withdraw_payload, |req| {
        player_a.sign_digest(req.digest)
    })
    .await?;
    log::info!("Step 8 (Withdraw) accepted: {withdraw_txid}");

    Ok(ScenarioReport {
        init_txid,
        deposit_a_txid,
        deposit_b_txid,
        create_game_txid,
        join_game_txid,
        turn_a_txid,
        turn_b_txid,
        withdraw_txid,
        player_a_pubkey: player_a.pubkey(),
        game_id,
        player_a_user_id,
        player_b_user_id,
    })
}

/// Helper for submitting signed lane action transactions with retry back-off.
async fn submit_action<C: RpcApi + ?Sized, S>(
    label: &str,
    ctx: &CarrierContext<'_, C>,
    payload: &LanePayload,
    sign: S,
) -> Result<Hash, String>
where
    S: Fn(vprogs_zk_backend_risc0_app_kit::SigRequest<'_>) -> [u8; 64],
{
    vprogs_zk_backend_risc0_app_kit::fund_and_submit(
        label,
        ctx.wallet,
        |candidates: Vec<(TransactionOutpoint, UtxoEntry)>| {
            let (outpoint, entry) = candidates.into_iter().next()?;
            let args = CarrierTxArgs {
                outpoint,
                entry,
                keypair: ctx.operator_keypair,
                change_address: ctx.wallet.address(),
                subnetwork_id: ctx.subnetwork_id,
                tx_version: TX_VERSION_TOCCATA,
                params: ctx.params,
                extra_outputs: vec![],
            };
            Some(signed_lane_action_tx(args, payload, &mut |req| sign(req)))
        },
        DEFAULT_MAX_SUBMIT_ATTEMPTS,
        DEFAULT_SUBMIT_RETRY_DELAY,
    )
    .await
    .ok_or_else(|| format!("{label}: transaction submission failed after retries"))
}

/// Helper for submitting signed deposit transactions with retry back-off.
async fn submit_deposit<C: RpcApi + ?Sized>(
    label: &str,
    ctx: &CarrierContext<'_, C>,
    payload: &LanePayload,
    covenant_id: &[u8; 32],
    deposit_amount: u64,
) -> Result<Hash, String> {
    let deposit_output = covenant_deposit_output(covenant_id, deposit_amount);
    vprogs_zk_backend_risc0_app_kit::fund_and_submit(
        label,
        ctx.wallet,
        |candidates: Vec<(TransactionOutpoint, UtxoEntry)>| {
            let (outpoint, entry) = candidates.into_iter().next()?;
            let args = CarrierTxArgs {
                outpoint,
                entry,
                keypair: ctx.operator_keypair,
                change_address: ctx.wallet.address(),
                subnetwork_id: ctx.subnetwork_id,
                tx_version: TX_VERSION_TOCCATA,
                params: ctx.params,
                extra_outputs: vec![deposit_output.clone()],
            };
            Some(signed_deposit_tx(args, payload))
        },
        DEFAULT_MAX_SUBMIT_ATTEMPTS,
        DEFAULT_SUBMIT_RETRY_DELAY,
    )
    .await
    .ok_or_else(|| format!("{label}: deposit submission failed after retries"))
}

#[cfg(test)]
mod tests {
    use vprog_tictactoe_guest::{
        program::action::{ActionBody, ActionTag, decode_action},
        runtime::ix::decode_ix,
    };

    use super::*;

    #[test]
    fn test_scenario_payloads_smoke_decode() {
        let covenant_id = [0x77u8; 32];
        let turn_ttl = 15_000u64;
        let genesis_lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &GENESIS_PUBKEY });

        let player_a = Bip340Signer::new();
        let player_b = Bip340Signer::new();

        let player_a_lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &player_a.pubkey() });
        let player_b_lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &player_b.pubkey() });

        let player_a_ilh = player_a_lock.id_hash();
        let player_b_ilh = player_b_lock.id_hash();

        let player_a_user_id = derive_user_resource(&player_a_ilh);
        let player_b_user_id = derive_user_resource(&player_b_ilh);
        let config_id = config_resource_id();
        let game_id = derive_game_resource(&player_a_ilh, 0);

        let rest_preimage = [0x55u8; 32];

        // 1. Init payload
        let p_init = build_init_payload(&covenant_id, 1_000_000, turn_ttl, &genesis_lock);
        let bytes_init = p_init.finish(&rest_preimage, &mut |req| player_a.sign_digest(req.digest));
        let mut slice_init = bytes_init.as_slice();
        let access_init = AccessMetadata::decode_vec(&mut slice_init).unwrap();
        assert_eq!(access_init.len(), 1);
        let ix_init = decode_ix(slice_init, access_init.len(), decode_action).unwrap();
        assert_eq!(ix_init.signers.len(), 1);
        assert_eq!(ix_init.signers[0].0, 0);
        assert_eq!(ix_init.actions.len(), 1);
        assert_eq!(ix_init.actions[0].action_tag, ActionTag::Init);

        // 2. Deposit payload
        let p_dep = build_deposit_payload(player_a_user_id, config_id, &player_a_lock);
        let bytes_dep = p_dep.finish_unsigned();
        let mut slice_dep = bytes_dep.as_slice();
        let access_dep = AccessMetadata::decode_vec(&mut slice_dep).unwrap();
        assert_eq!(access_dep.len(), 2);
        assert!(access_dep[0].resource_id < access_dep[1].resource_id);
        let ix_dep = decode_ix(slice_dep, access_dep.len(), decode_action).unwrap();
        assert_eq!(ix_dep.signers.len(), 0);
        assert_eq!(ix_dep.actions.len(), 1);
        assert_eq!(ix_dep.actions[0].action_tag, ActionTag::Deposit);

        // 3. CreateGame payload
        let p_create = build_create_game_payload(player_a_user_id, game_id, 50_000_000, 1, Cell::X);
        let bytes_create =
            p_create.finish(&rest_preimage, &mut |req| player_a.sign_digest(req.digest));
        let mut slice_create = bytes_create.as_slice();
        let access_create = AccessMetadata::decode_vec(&mut slice_create).unwrap();
        assert_eq!(access_create.len(), 2);
        assert!(access_create[0].resource_id < access_create[1].resource_id);
        let ix_create = decode_ix(slice_create, access_create.len(), decode_action).unwrap();
        assert_eq!(ix_create.signers.len(), 1);
        assert_eq!(ix_create.actions.len(), 1);
        assert_eq!(ix_create.actions[0].action_tag, ActionTag::CreateGame);

        // 4. JoinGame payload
        let p_join = build_join_game_payload(player_b_user_id, game_id);
        let bytes_join = p_join.finish(&rest_preimage, &mut |req| player_b.sign_digest(req.digest));
        let mut slice_join = bytes_join.as_slice();
        let access_join = AccessMetadata::decode_vec(&mut slice_join).unwrap();
        assert_eq!(access_join.len(), 2);
        assert!(access_join[0].resource_id < access_join[1].resource_id);
        let ix_join = decode_ix(slice_join, access_join.len(), decode_action).unwrap();
        assert_eq!(ix_join.signers.len(), 1);
        assert_eq!(ix_join.actions.len(), 1);
        assert_eq!(ix_join.actions[0].action_tag, ActionTag::JoinGame);

        // 5. Turn A payload (3 actions)
        let p_turn_a = build_turn_a_payload(game_id, player_a_user_id, player_b_user_id);
        let bytes_turn_a =
            p_turn_a.finish(&rest_preimage, &mut |req| player_a.sign_digest(req.digest));
        let mut slice_turn_a = bytes_turn_a.as_slice();
        let access_turn_a = AccessMetadata::decode_vec(&mut slice_turn_a).unwrap();
        assert_eq!(access_turn_a.len(), 3);
        assert!(access_turn_a[0].resource_id < access_turn_a[1].resource_id);
        assert!(access_turn_a[1].resource_id < access_turn_a[2].resource_id);
        let ix_turn_a = decode_ix(slice_turn_a, access_turn_a.len(), decode_action).unwrap();
        assert_eq!(ix_turn_a.signers.len(), 1);
        assert_eq!(ix_turn_a.actions.len(), 3);
        assert_eq!(ix_turn_a.actions[0].action_tag, ActionTag::Turn);
        assert_eq!(ix_turn_a.actions[1].action_tag, ActionTag::Turn);
        assert_eq!(ix_turn_a.actions[2].action_tag, ActionTag::Turn);
        match &ix_turn_a.actions[0].body {
            ActionBody::Turn { cell, .. } => assert_eq!(*cell, 0),
            _ => panic!("expected Turn action"),
        }
        match &ix_turn_a.actions[1].body {
            ActionBody::Turn { cell, .. } => assert_eq!(*cell, 1),
            _ => panic!("expected Turn action"),
        }
        match &ix_turn_a.actions[2].body {
            ActionBody::Turn { cell, .. } => assert_eq!(*cell, 2),
            _ => panic!("expected Turn action"),
        }

        // 6. Turn B payload (2 actions)
        let p_turn_b = build_turn_b_payload(game_id, player_a_user_id, player_b_user_id);
        let bytes_turn_b =
            p_turn_b.finish(&rest_preimage, &mut |req| player_b.sign_digest(req.digest));
        let mut slice_turn_b = bytes_turn_b.as_slice();
        let access_turn_b = AccessMetadata::decode_vec(&mut slice_turn_b).unwrap();
        assert_eq!(access_turn_b.len(), 3);
        let ix_turn_b = decode_ix(slice_turn_b, access_turn_b.len(), decode_action).unwrap();
        assert_eq!(ix_turn_b.signers.len(), 1);
        assert_eq!(ix_turn_b.actions.len(), 2);
        assert_eq!(ix_turn_b.actions[0].action_tag, ActionTag::Turn);
        assert_eq!(ix_turn_b.actions[1].action_tag, ActionTag::Turn);
        match &ix_turn_b.actions[0].body {
            ActionBody::Turn { cell, .. } => assert_eq!(*cell, 3),
            _ => panic!("expected Turn action"),
        }
        match &ix_turn_b.actions[1].body {
            ActionBody::Turn { cell, .. } => assert_eq!(*cell, 4),
            _ => panic!("expected Turn action"),
        }

        // 7. Withdraw payload
        let dest = StandardSpk::PubKey(&player_a.pubkey());
        let p_withdraw = build_withdraw_payload(player_a_user_id, config_id, 100_000_000, &dest);
        let bytes_withdraw =
            p_withdraw.finish(&rest_preimage, &mut |req| player_a.sign_digest(req.digest));
        let mut slice_withdraw = bytes_withdraw.as_slice();
        let access_withdraw = AccessMetadata::decode_vec(&mut slice_withdraw).unwrap();
        assert_eq!(access_withdraw.len(), 2);
        let ix_withdraw = decode_ix(slice_withdraw, access_withdraw.len(), decode_action).unwrap();
        assert_eq!(ix_withdraw.signers.len(), 1);
        assert_eq!(ix_withdraw.actions.len(), 1);
        assert_eq!(ix_withdraw.actions[0].action_tag, ActionTag::Withdraw);
    }
}
