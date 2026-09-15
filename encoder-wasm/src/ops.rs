//! Transaction builders per demo operation: thin composition of the guest encoders with the
//! app-kit carrier and claim assembly, mirroring the driver's carrier composition.

use std::str::FromStr;

use kaspa_addresses::Address;
use kaspa_consensus_core::{
    constants::{STORAGE_MASS_PARAMETER, TX_VERSION_TOCCATA},
    mass::{UtxoCell, UtxoPlurality, calc_storage_mass},
    subnets::SubnetworkId,
    tx::{ScriptPublicKey, Transaction, TransactionOutpoint, TransactionOutput, UtxoEntry},
};
use kaspa_hashes::Hash;
use secp256k1::{Keypair, SECP256K1, SecretKey};
use vprog_tictactoe_guest::{
    program::{
        action::encode::{
            encode_create_game_action, encode_deposit_action, encode_join_game_action,
            encode_transfer_action, encode_transfer_create_action, encode_turn_action,
            encode_withdraw_action, game_two_user_access, game_user_access, two_user_access,
            user_config_access,
        },
        resources::{game::Cell, id::derive_game_resource},
    },
    runtime::lock::{LockEnum, SchnorrLockView},
};
use vprogs_core_types::{AccessMetadata, ResourceId};
use vprogs_zk_abi::withdrawal::StandardSpk;
use vprogs_zk_backend_risc0_app_kit::{
    Bip340Signer, CarrierTxArgs, LanePayload, SchnorrSigPtrSigner, SignerKind, SignerSpec,
    TailBlock,
    claim::{PermissionSpendArgs, build_permission_spend},
    covenant_deposit_output, signed_lane_action_tx,
    signer::Signer,
};
use wasm_bindgen::prelude::*;

use crate::{JsParams, UtxoCandidate};

/// Builds one signed carrier over `payload`, funded by the single `utxo`, mirroring the driver's
/// `CarrierTxArgs` composition (`TX_VERSION_TOCCATA`, extra outputs before change).
fn signed_carrier(
    net: &JsParams,
    identity: &Identity,
    utxo: &UtxoCandidate,
    change_address: &str,
    lane_subnet_hex: &str,
    payload: LanePayload,
    extra_outputs: Vec<TransactionOutput>,
) -> Result<Transaction, JsError> {
    let change =
        Address::try_from(change_address).map_err(|_| JsError::new("invalid change address"))?;
    let subnet = SubnetworkId::from_bytes(hex20(lane_subnet_hex)?);
    let (outpoint, entry) = funding(utxo)?;
    let args = CarrierTxArgs {
        outpoint,
        entry,
        keypair: identity.keypair,
        change_address: &change,
        subnetwork_id: subnet,
        tx_version: TX_VERSION_TOCCATA,
        params: &net.params,
        extra_outputs,
    };
    Ok(signed_lane_action_tx(args, &payload, &mut |req| identity.signer.sign_digest(req.digest)))
}

/// The signing identity derived from one private key: the payload signer, the L1 input funder,
/// and the user identity its schnorr lock derives.
pub(crate) struct Identity {
    /// BIP-340 payload signer whose x-only pubkey forms the schnorr lock.
    signer: Bip340Signer,
    /// Keypair signing (and funding) the carrier input.
    keypair: Keypair,
    /// The lock's x-only public key, held so the lock can borrow it.
    pubkey: [u8; 32],
}

impl Identity {
    /// Derives the identity from a 32-byte private key hex string.
    pub(crate) fn from_privkey_hex(privkey_hex: &str) -> Result<Self, JsError> {
        let secret_key = SecretKey::from_slice(&hex32(privkey_hex)?)
            .map_err(|_| JsError::new("invalid secp256k1 private key"))?;
        let signer = Bip340Signer::from_secret_key(&secret_key);
        let pubkey = signer.pubkey();
        Ok(Self { keypair: Keypair::from_secret_key(SECP256K1, &secret_key), signer, pubkey })
    }

    /// The schnorr lock view over this identity's x-only public key.
    fn lock(&self) -> LockEnum<'_> {
        LockEnum::Schnorr(SchnorrLockView { pubkey: &self.pubkey })
    }

    /// The lock's identity hash.
    pub(crate) fn lock_hash(&self) -> [u8; 32] {
        self.lock().id_hash()
    }

    /// The x-only public key bytes.
    pub(crate) fn pubkey(&self) -> &[u8; 32] {
        &self.pubkey
    }

    /// The user resource this identity's lock derives.
    pub(crate) fn user_id(&self) -> ResourceId {
        vprog_tictactoe_guest::program::resources::id::derive_user_resource(&self.lock_hash())
    }
}

/// Rebuilds the funding outpoint and its UTXO entry from a wallet-fetched candidate.
fn funding(utxo: &UtxoCandidate) -> Result<(TransactionOutpoint, UtxoEntry), JsError> {
    let txid = Hash::from_str(&utxo.txid_hex).map_err(|_| JsError::new("invalid funding txid"))?;
    let spk = ScriptPublicKey::new(utxo.spk_version, hex_vec(&utxo.spk_hex)?.into());
    Ok((
        TransactionOutpoint::new(txid, utxo.index),
        UtxoEntry::new(utxo.amount, spk, 0, false, None),
    ))
}

/// The signer spec every signed demo action uses: a schnorr sig-pointer over the acting user.
fn schnorr_spec(resource_idx: u8) -> SignerSpec {
    SignerSpec {
        resource_idx,
        kind: SignerKind::SigPtr { tag: SchnorrSigPtrSigner::TAG },
        tail: TailBlock::Sig64,
    }
}

/// Sorts and dedups two access lists into the strictly-ascending merged list the ABI requires.
fn merge_access(mut access: Vec<AccessMetadata>, more: Vec<AccessMetadata>) -> Vec<AccessMetadata> {
    access.extend(more);
    access.sort_by_key(|m| m.resource_id);
    access.dedup_by(|a, b| a.resource_id == b.resource_id);
    access
}

/// Returns `id`'s position in `access`, which construction guarantees it holds.
fn position(access: &[AccessMetadata], id: &ResourceId) -> u8 {
    access
        .iter()
        .position(|m| &m.resource_id == id)
        .expect("merged access holds every action resource") as u8
}

/// Decodes a 64-char hex string into 32 bytes.
fn hex32(s: &str) -> Result<[u8; 32], JsError> {
    let mut out = [0u8; 32];
    faster_hex::hex_decode(s.as_bytes(), &mut out)
        .map_err(|_| JsError::new("expected a 64-char hex string"))?;
    Ok(out)
}

/// Decodes a 40-char hex string into the 20-byte lane subnetwork id.
fn hex20(s: &str) -> Result<[u8; 20], JsError> {
    let mut out = [0u8; 20];
    faster_hex::hex_decode(s.as_bytes(), &mut out)
        .map_err(|_| JsError::new("expected a 40-char lane subnet hex string"))?;
    Ok(out)
}

/// Decodes an even-length hex string into bytes.
fn hex_vec(s: &str) -> Result<Vec<u8>, JsError> {
    if !s.len().is_multiple_of(2) {
        return Err(JsError::new("expected an even-length hex string"));
    }
    let mut out = vec![0u8; s.len() / 2];
    faster_hex::hex_decode(s.as_bytes(), &mut out)
        .map_err(|_| JsError::new("invalid hex string"))?;
    Ok(out)
}

/// Decodes a 64-char hex string into a resource id.
fn resource_id(s: &str) -> Result<ResourceId, JsError> {
    Ok(ResourceId::from(hex32(s)?))
}

/// Serializes a built transaction to borsh bytes ready for `submitTransaction`.
fn borsh_bytes(tx: &Transaction) -> Result<Vec<u8>, JsError> {
    borsh::to_vec(tx).map_err(|_| JsError::new("transaction serialization failed"))
}

/// Maps a mark byte to the game mark, mirroring the guest decoder (X = 1, O = 2).
fn cell_from_u8(mark: u8) -> Result<Cell, JsError> {
    match mark {
        1 => Ok(Cell::X),
        2 => Ok(Cell::O),
        _ => Err(JsError::new("mark must be 1 (X) or 2 (O)")),
    }
}

/// Builds a signed `CreateGame` carrier; a positive `deposit_amount` prepends a `Deposit` action
/// citing the covenant deposit output this transaction carries at index 0.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
#[allow(clippy::too_many_arguments)]
pub fn create_game_tx(
    privkey_hex: &str,
    net: &JsParams,
    utxo: &UtxoCandidate,
    change_address: &str,
    lane_subnet_hex: &str,
    config_id_hex: &str,
    my_games_started: u64,
    stake: u64,
    rounds: u8,
    mark_u8: u8,
    deposit_amount: u64,
    covenant_id_hex: &str,
) -> Result<Vec<u8>, JsError> {
    let identity = Identity::from_privkey_hex(privkey_hex)?;
    let mark = cell_from_u8(mark_u8)?;
    let game_id = derive_game_resource(&identity.lock_hash(), my_games_started);
    let game_access = game_user_access(game_id, identity.user_id());
    let config_id = resource_id(config_id_hex)?;

    let mut payload = LanePayload::new();
    let mut extra_outputs = Vec::new();
    if deposit_amount > 0 {
        let covenant_id = hex32(covenant_id_hex)?;
        if utxo.amount <= deposit_amount {
            return Err(JsError::new("funding UTXO does not cover the deposit"));
        }
        let access = merge_access(
            user_config_access(identity.user_id(), config_id).access,
            game_access.access,
        );
        let user_idx = position(&access, &identity.user_id());
        let deposit =
            encode_deposit_action(user_idx, position(&access, &config_id), 0, &identity.lock());
        let create =
            encode_create_game_action(user_idx, position(&access, &game_id), stake, rounds, mark);
        for entry in access {
            payload = payload.access(entry);
        }
        payload = payload.action(deposit).action(create).signer(schnorr_spec(user_idx));
        extra_outputs.push(covenant_deposit_output(&covenant_id, deposit_amount));
    } else {
        payload = payload
            .access(game_access.access[0])
            .access(game_access.access[1])
            .action(encode_create_game_action(
                game_access.user_idx,
                game_access.game_idx,
                stake,
                rounds,
                mark,
            ))
            .signer(schnorr_spec(game_access.user_idx));
    }

    let tx = signed_carrier(
        net,
        &identity,
        utxo,
        change_address,
        lane_subnet_hex,
        payload,
        extra_outputs,
    )?;
    borsh_bytes(&tx)
}

/// Builds a signed `JoinGame` carrier; a positive `deposit_amount` prepends a `Deposit` action
/// citing the covenant deposit output this transaction carries at index 0.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
#[allow(clippy::too_many_arguments)]
pub fn join_game_tx(
    privkey_hex: &str,
    net: &JsParams,
    utxo: &UtxoCandidate,
    change_address: &str,
    lane_subnet_hex: &str,
    config_id_hex: &str,
    game_id_hex: &str,
    deposit_amount: u64,
    covenant_id_hex: &str,
) -> Result<Vec<u8>, JsError> {
    let identity = Identity::from_privkey_hex(privkey_hex)?;
    let game_id = resource_id(game_id_hex)?;
    let game_access = game_user_access(game_id, identity.user_id());

    let mut payload = LanePayload::new();
    let mut extra_outputs = Vec::new();
    if deposit_amount > 0 {
        let covenant_id = hex32(covenant_id_hex)?;
        if utxo.amount <= deposit_amount {
            return Err(JsError::new("funding UTXO does not cover the deposit"));
        }
        let config_id = resource_id(config_id_hex)?;
        let access = merge_access(
            user_config_access(identity.user_id(), config_id).access,
            game_access.access,
        );
        let user_idx = position(&access, &identity.user_id());
        let deposit =
            encode_deposit_action(user_idx, position(&access, &config_id), 0, &identity.lock());
        let join = encode_join_game_action(position(&access, &game_id), user_idx);
        for entry in access {
            payload = payload.access(entry);
        }
        payload = payload.action(deposit).action(join).signer(schnorr_spec(user_idx));
        extra_outputs.push(covenant_deposit_output(&covenant_id, deposit_amount));
    } else {
        payload = payload
            .access(game_access.access[0])
            .access(game_access.access[1])
            .action(encode_join_game_action(game_access.game_idx, game_access.user_idx))
            .signer(schnorr_spec(game_access.user_idx));
    }

    let tx = signed_carrier(
        net,
        &identity,
        utxo,
        change_address,
        lane_subnet_hex,
        payload,
        extra_outputs,
    )?;
    borsh_bytes(&tx)
}

/// Builds a signed single-`Turn` carrier playing `cell` (0..=8) in the caller's game.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
#[allow(clippy::too_many_arguments)]
pub fn turn_tx(
    privkey_hex: &str,
    net: &JsParams,
    utxo: &UtxoCandidate,
    change_address: &str,
    lane_subnet_hex: &str,
    game_id_hex: &str,
    my_user_id_hex: &str,
    opponent_user_id_hex: &str,
    cell: u8,
) -> Result<Vec<u8>, JsError> {
    if cell > 8 {
        return Err(JsError::new("cell must be within 0..=8"));
    }
    let identity = Identity::from_privkey_hex(privkey_hex)?;
    let access = game_two_user_access(
        resource_id(game_id_hex)?,
        resource_id(my_user_id_hex)?,
        resource_id(opponent_user_id_hex)?,
    );
    let payload = LanePayload::new()
        .access(access.access[0])
        .access(access.access[1])
        .access(access.access[2])
        .action(encode_turn_action(access.game_idx, access.first_user_idx, cell))
        .signer(schnorr_spec(access.first_user_idx));

    let tx =
        signed_carrier(net, &identity, utxo, change_address, lane_subnet_hex, payload, vec![])?;
    borsh_bytes(&tx)
}

/// Builds a signed `Transfer` carrier from the caller's user to `dest_user_id_hex`; a new
/// destination additionally requires its x-only public key to bind its lock.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
#[allow(clippy::too_many_arguments)]
pub fn transfer_tx(
    privkey_hex: &str,
    net: &JsParams,
    utxo: &UtxoCandidate,
    change_address: &str,
    lane_subnet_hex: &str,
    dest_user_id_hex: &str,
    dest_exists: bool,
    amount: u64,
    dest_pubkey_hex: Option<String>,
) -> Result<Vec<u8>, JsError> {
    let identity = Identity::from_privkey_hex(privkey_hex)?;
    let access = two_user_access(identity.user_id(), resource_id(dest_user_id_hex)?);
    let action = if dest_exists {
        encode_transfer_action(access.source_idx, access.dest_idx, amount)
    } else {
        let dest_pubkey = hex32(dest_pubkey_hex.as_deref().ok_or_else(|| {
            JsError::new("dest_pubkey_hex is required when the destination does not exist")
        })?)?;
        encode_transfer_create_action(
            access.source_idx,
            access.dest_idx,
            amount,
            &LockEnum::Schnorr(SchnorrLockView { pubkey: &dest_pubkey }),
        )
    };
    let payload = LanePayload::new()
        .access(access.access[0])
        .access(access.access[1])
        .action(action)
        .signer(schnorr_spec(access.source_idx));

    let tx =
        signed_carrier(net, &identity, utxo, change_address, lane_subnet_hex, payload, vec![])?;
    borsh_bytes(&tx)
}

/// Builds a signed `Withdraw` carrier exiting `amount` to the caller's own public key.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn withdraw_tx(
    privkey_hex: &str,
    net: &JsParams,
    utxo: &UtxoCandidate,
    change_address: &str,
    lane_subnet_hex: &str,
    config_id_hex: &str,
    amount: u64,
) -> Result<Vec<u8>, JsError> {
    let identity = Identity::from_privkey_hex(privkey_hex)?;
    let access = user_config_access(identity.user_id(), resource_id(config_id_hex)?);
    let dest = StandardSpk::PubKey(identity.pubkey());
    let payload = LanePayload::new()
        .access(access.access[0])
        .access(access.access[1])
        .action(encode_withdraw_action(access.user_idx, access.config_idx, amount, &dest))
        .signer(schnorr_spec(access.user_idx));

    let tx =
        signed_carrier(net, &identity, utxo, change_address, lane_subnet_hex, payload, vec![])?;
    borsh_bytes(&tx)
}

/// Builds a full-leaf permission-tree claim spending the settled exit leaf; `delegate_utxos` are
/// covenant deposit UTXOs funding the payout, and `fee` burns delegate value for relay priority.
///
/// Fee edge: `fee` is subtracted from the trailing delegate change output, so delegate inputs
/// summing to exactly `deduct + fee` drive that change to zero value, which the network rejects as
/// dust. Unreachable in the demo (simnet fee is 0); nonzero-fee callers must overfund the delegate
/// pool past `deduct + fee`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
#[allow(clippy::too_many_arguments)]
pub fn claim_tx(
    covenant_id_hex: &str,
    permission_txid_hex: &str,
    permission_index: u32,
    permission_rent: u64,
    old_root_hex: &str,
    old_unclaimed: u64,
    depth: u32,
    leaf_index: u32,
    leaf_spk_hex: &str,
    leaf_amount: u64,
    new_root_hex: &str,
    new_unclaimed: u64,
    siblings_hex: Vec<String>,
    delegate_utxos: Vec<UtxoCandidate>,
    fee: u64,
) -> Result<Vec<u8>, JsError> {
    let covenant_id = hex32(covenant_id_hex)?;
    let permission_txid =
        Hash::from_str(permission_txid_hex).map_err(|_| JsError::new("invalid permission txid"))?;
    let siblings =
        siblings_hex.iter().map(|s| hex32(s)).collect::<Result<Vec<[u8; 32]>, JsError>>()?;
    let delegate_inputs = delegate_utxos
        .iter()
        .map(|utxo| {
            let txid = Hash::from_str(&utxo.txid_hex)
                .map_err(|_| JsError::new("invalid delegate txid"))?;
            Ok((TransactionOutpoint::new(txid, utxo.index), utxo.amount))
        })
        .collect::<Result<Vec<(TransactionOutpoint, u64)>, JsError>>()?;

    // The demo locks claims to full-leaf deduct; delegate value must cover payout plus fee.
    let deduct = leaf_amount;
    let total_delegate = delegate_inputs.iter().try_fold(0u64, |acc, (_, amount)| {
        acc.checked_add(*amount).ok_or_else(|| JsError::new("delegate total overflows"))
    })?;
    let required =
        deduct.checked_add(fee).ok_or_else(|| JsError::new("deduct plus fee overflows"))?;
    if total_delegate < required {
        return Err(JsError::new("delegate inputs do not cover the payout plus fee"));
    }

    let leaf_spk = hex_vec(leaf_spk_hex)?;
    let delegate_amounts: Vec<u64> = delegate_inputs.iter().map(|(_, amount)| *amount).collect();
    let args = PermissionSpendArgs {
        covenant_id,
        permission_outpoint: TransactionOutpoint::new(permission_txid, permission_index),
        permission_rent,
        old_root: hex32(old_root_hex)?,
        old_unclaimed,
        depth: depth as usize,
        leaf_index: leaf_index as usize,
        leaf_spk: &leaf_spk,
        leaf_amount,
        deduct,
        siblings,
        new_root: hex32(new_root_hex)?,
        new_unclaimed,
        delegate_inputs,
    };
    let (mut tx, utxos) = build_permission_spend(&args).map_err(JsError::new)?;

    // Burn `fee` from the trailing delegate change output (inputs minus outputs).
    if fee > 0 {
        let change = tx
            .outputs
            .last_mut()
            .ok_or_else(|| JsError::new("claim tx has no output to pay the fee from"))?;
        change.value -= fee;
    }

    // Commit the KIP-0009 storage mass (Toccata txs must carry it or the node disqualifies
    // their block). Cells must mirror the consensus pluralities exactly: the covenant-bound
    // permission input and continuation output occupy two 100-byte storage units each, while
    // the delegate inputs and the plain outputs occupy one. STORAGE_MASS_PARAMETER is uniform
    // across every network preset (a custom ParamsOverrides could diverge; out of demo scope).
    let permission_entry = utxos.first().expect("claim builder returns the permission entry");
    let input_cells: Vec<UtxoCell> =
        std::iter::once(UtxoCell::new(permission_entry.plurality(), permission_rent))
            .chain(delegate_amounts.into_iter().map(|amount| UtxoCell::new(1, amount)))
            .collect();
    let output_cells = tx.outputs.iter().map(|o| UtxoCell::new(o.plurality(), o.value));
    let storage_mass =
        calc_storage_mass(false, input_cells.iter().copied(), output_cells, STORAGE_MASS_PARAMETER)
            .ok_or_else(|| JsError::new("storage mass calculation overflowed"))?;
    tx.set_storage_mass(storage_mass);
    borsh_bytes(&tx)
}
