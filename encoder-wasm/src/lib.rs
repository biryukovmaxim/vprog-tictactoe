//! Pure build-and-sign surface for the web demo over the guest encoders and app-kit.
//!
//! Every exported function is deterministic: no fetch, no websocket, no clock. The browser owns
//! IO (keys via kaspa-wasm, UTXOs and submission via RPC, config via the DA) and passes values
//! in; the returned borsh transaction bytes are ready for `submitTransaction`.
//!
//! The wasm-bindgen attributes are gated on `cfg(target_arch = "wasm32")` so the same crate
//! builds and tests on the host as a plain library.

mod ops;

use std::str::FromStr;

use kaspa_addresses::Prefix;
use kaspa_consensus_core::{config::params::Params, network::NetworkId};
pub use ops::{claim_tx, create_game_tx, join_game_tx, transfer_tx, turn_tx, withdraw_tx};
use vprog_tictactoe_guest::{
    program::resources::id::derive_user_resource,
    runtime::lock::{LockEnum, SchnorrLockView},
};
use wasm_bindgen::prelude::*;

/// One spendable L1 UTXO backing a carrier input, as fetched from the wallet.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter_with_clone))]
#[derive(Clone)]
pub struct UtxoCandidate {
    /// Transaction id of the funding output, in display hex form.
    pub txid_hex: String,
    /// Output index within the funding transaction.
    pub index: u32,
    /// Output value in sompi.
    pub amount: u64,
    /// Output script public key bytes in hex.
    pub spk_hex: String,
    /// Output script public key version.
    pub spk_version: u16,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl UtxoCandidate {
    /// Assembles a funding-output reference for a carrier input.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(constructor))]
    pub fn new(
        txid_hex: String,
        index: u32,
        amount: u64,
        spk_hex: String,
        spk_version: u16,
    ) -> Self {
        Self { txid_hex, index, amount, spk_hex, spk_version }
    }
}

/// Parsed network parameters, shared across every tx-builder call.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct JsParams {
    params: Params,
    prefix: Prefix,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl JsParams {
    /// Address prefix string for this network (e.g. `kaspasim`).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter, js_name = "prefix"))]
    pub fn prefix_str(&self) -> String {
        self.prefix.to_string()
    }
}

/// The caller's derived on-rollup identity for one private key.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter_with_clone))]
pub struct MyIds {
    /// Schnorr lock identity hash hex the user resource derives from.
    pub lock_hash_hex: String,
    /// User resource id hex.
    pub user_id_hex: String,
    /// X-only public key hex.
    pub pubkey_hex: String,
}

/// Parses a network name (`simnet`, `testnet-10`) into shared consensus parameters.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn network_params(network: &str) -> Result<JsParams, JsError> {
    let network_id =
        NetworkId::from_str(network).map_err(|_| JsError::new("unsupported network name"))?;
    let params = Params::from(network_id);
    let prefix = Prefix::from(params.net.network_type());
    Ok(JsParams { params, prefix })
}

/// Derives the lock hash, user resource id, and x-only public key for a private key hex.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn my_ids(privkey_hex: &str) -> Result<MyIds, JsError> {
    let identity = ops::Identity::from_privkey_hex(privkey_hex)?;
    let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: identity.pubkey() });
    let lock_hash = lock.id_hash();
    Ok(MyIds {
        lock_hash_hex: faster_hex::hex_string(&lock_hash),
        user_id_hex: faster_hex::hex_string(derive_user_resource(&lock_hash).as_slice()),
        pubkey_hex: faster_hex::hex_string(identity.pubkey()),
    })
}

#[cfg(test)]
mod tests {
    use borsh::BorshDeserialize;
    use kaspa_addresses::{Address, Prefix, Version};
    use kaspa_consensus_core::{
        config::params::SIMNET_PARAMS,
        constants::TX_VERSION_TOCCATA,
        hashing::sighash::SigHashReusedValuesUnsync,
        mass::MassCalculator,
        tx::{PopulatedTransaction, ScriptPublicKey, Transaction, UtxoEntry},
    };
    use kaspa_hashes::Hash;
    use kaspa_txscript::{
        EngineFlags, TxScriptEngine, caches::Cache, covenants::CovenantsContext,
        engine_context::EngineContext, pay_to_address_script,
        seq_commit_accessor::SeqCommitAccessor, standard::pay_to_script_hash_script,
    };
    use secp256k1::SecretKey;
    use vprog_tictactoe_guest::{
        program::{
            action::{ActionBody, ActionTag, ActionView, decode_action},
            resources::{
                game::Cell,
                id::{config_resource_id, derive_game_resource, derive_user_resource},
            },
        },
        runtime::{
            ix::{DecodedIx, decode_ix},
            lock::{LockEnum, SchnorrLockView},
        },
    };
    use vprogs_core_types::{AccessMetadata, ResourceId};
    use vprogs_zk_abi::withdrawal::{ExitLeaf, StandardSpk};
    use vprogs_zk_backend_risc0_api::{
        PermissionTreeAccumulator, build_delegate_entry_script, build_permission_redeem_script,
    };
    use vprogs_zk_backend_risc0_app_kit::{
        Bip340Signer,
        claim::{PermissionTreeView, claim_siblings},
        covenant_deposit_output,
    };

    use super::*;

    /// Deterministic test key: secp256k1 scalar 7.
    fn test_secret() -> [u8; 32] {
        let mut secret = [0u8; 32];
        secret[31] = 7;
        secret
    }

    fn test_privkey_hex() -> String {
        faster_hex::hex_string(&test_secret())
    }

    fn test_signer() -> Bip340Signer {
        Bip340Signer::from_secret_key(&SecretKey::from_slice(&test_secret()).unwrap())
    }

    fn test_net() -> JsParams {
        network_params("simnet").unwrap()
    }

    fn test_address(signer: &Bip340Signer) -> Address {
        Address::new(Prefix::Simnet, Version::PubKey, &signer.pubkey())
    }

    fn test_utxo(address: &Address) -> UtxoCandidate {
        let spk = pay_to_address_script(address);
        UtxoCandidate {
            txid_hex: Hash::from_bytes([0x21u8; 32]).to_string(),
            index: 3,
            amount: 1_000_000_000,
            spk_hex: faster_hex::hex_string(spk.script()),
            spk_version: spk.version(),
        }
    }

    fn hex32_str(bytes: &[u8; 32]) -> String {
        faster_hex::hex_string(bytes)
    }

    fn lane_subnet_hex() -> String {
        faster_hex::hex_string(&[0x33u8; 20])
    }

    fn decode_built(bytes: &[u8]) -> Transaction {
        Transaction::deserialize(&mut &bytes[..]).unwrap()
    }

    fn decode_lane(tx: &Transaction) -> (Vec<AccessMetadata>, DecodedIx<ActionView<'_>>) {
        let mut slice = tx.payload.as_slice();
        let access = AccessMetadata::decode_vec(&mut slice).unwrap();
        let ix = decode_ix(slice, access.len(), decode_action).unwrap();
        (access, ix)
    }

    #[test]
    fn network_params_maps_supported_networks() {
        assert_eq!(network_params("simnet").unwrap().prefix_str(), "kaspasim");
        assert_eq!(network_params("testnet-10").unwrap().prefix_str(), "kaspatest");
        // Error paths construct `JsError`, which only exists on wasm32.
        #[cfg(target_arch = "wasm32")]
        assert!(network_params("frobnet").is_err());
    }

    #[test]
    fn my_ids_derives_schnorr_lock_identity() {
        let signer = test_signer();
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let lock_hash = lock.id_hash();

        let ids = my_ids(&test_privkey_hex()).unwrap();
        assert_eq!(ids.pubkey_hex, faster_hex::hex_string(&signer.pubkey()));
        assert_eq!(ids.lock_hash_hex, hex32_str(&lock_hash));
        assert_eq!(ids.user_id_hex, hex32_str(&derive_user_resource(&lock_hash)));
    }

    #[test]
    fn create_game_tx_unfunded_single_action() {
        let signer = test_signer();
        let net = test_net();
        let address = test_address(&signer);
        let utxo = test_utxo(&address);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let lock_hash = lock.id_hash();
        let user_id = derive_user_resource(&lock_hash);
        let game_id = derive_game_resource(&lock_hash, 0);

        let bytes = create_game_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&config_resource_id()),
            0,
            50_000_000,
            3,
            1,
            0,
            &hex32_str(&[0x77u8; 32]),
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);
        assert_eq!(tx.version, TX_VERSION_TOCCATA);

        let (access, ix) = decode_lane(&tx);
        assert_eq!(access.len(), 2);
        assert!(access[0].resource_id < access[1].resource_id);
        assert_eq!(ix.actions.len(), 1);
        assert_eq!(ix.actions[0].action_tag, ActionTag::CreateGame);
        match &ix.actions[0].body {
            ActionBody::CreateGame { creator_idx, game_idx, stake, rounds, mark } => {
                assert_eq!(access[*creator_idx as usize].resource_id, user_id);
                assert_eq!(access[*game_idx as usize].resource_id, game_id);
                assert_eq!(*stake, 50_000_000);
                assert_eq!(*rounds, 3);
                assert_eq!(*mark, Cell::X);
            }
            _ => panic!("expected create game action"),
        }
        assert_eq!(ix.signers.len(), 1);
        assert_eq!(access[ix.signers[0].0 as usize].resource_id, user_id);
        assert_eq!(tx.outputs.len(), 1);
    }

    #[test]
    fn create_game_tx_funded_prepends_deposit() {
        let signer = test_signer();
        let net = test_net();
        let address = test_address(&signer);
        let utxo = test_utxo(&address);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let lock_hash = lock.id_hash();
        let user_id = derive_user_resource(&lock_hash);
        let covenant_id = [0x77u8; 32];
        let deposit = 25_000_000u64;

        let bytes = create_game_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&config_resource_id()),
            0,
            50_000_000,
            3,
            1,
            deposit,
            &hex32_str(&covenant_id),
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);

        // Deposit funding output sits at index 0, cited by the Deposit action.
        let want = covenant_deposit_output(&covenant_id, deposit);
        assert_eq!(tx.outputs[0].value, want.value);
        assert_eq!(tx.outputs[0].script_public_key.script(), want.script_public_key.script());

        let (access, ix) = decode_lane(&tx);
        assert_eq!(access.len(), 3);
        assert!(access[0].resource_id < access[1].resource_id);
        assert!(access[1].resource_id < access[2].resource_id);
        assert_eq!(ix.actions.len(), 2);
        assert_eq!(ix.actions[0].action_tag, ActionTag::Deposit);
        assert_eq!(ix.actions[1].action_tag, ActionTag::CreateGame);
        match &ix.actions[0].body {
            ActionBody::Deposit { user_idx, config_idx, output_idx, initial_lock } => {
                assert_eq!(*output_idx, 0);
                assert_eq!(access[*user_idx as usize].resource_id, user_id);
                assert_eq!(access[*config_idx as usize].resource_id, config_resource_id());
                assert_eq!(initial_lock.id_hash(), lock_hash);
            }
            _ => panic!("expected deposit action"),
        }
        // Fee sanity: outputs sum below the funding amount.
        assert!(tx.outputs[0].value + tx.outputs[1].value < utxo.amount);
    }

    #[test]
    fn join_game_tx_plain_and_funded() {
        let signer = test_signer();
        let net = test_net();
        let address = test_address(&signer);
        let utxo = test_utxo(&address);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let user_id = derive_user_resource(&lock.id_hash());
        let game_id = ResourceId::from([0x42u8; 32]);
        let covenant_id = [0x77u8; 32];
        let deposit = 10_000_000u64;

        // Plain join: single action.
        let bytes = join_game_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&config_resource_id()),
            &hex32_str(&game_id),
            0,
            &hex32_str(&covenant_id),
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);
        let (access, ix) = decode_lane(&tx);
        assert_eq!(access.len(), 2);
        assert_eq!(ix.actions.len(), 1);
        match &ix.actions[0].body {
            ActionBody::JoinGame { game_idx, joiner_idx } => {
                assert_eq!(access[*game_idx as usize].resource_id, game_id);
                assert_eq!(access[*joiner_idx as usize].resource_id, user_id);
            }
            _ => panic!("expected join game action"),
        }

        // Funded join: [Deposit, JoinGame] with the covenant output at index 0.
        let bytes = join_game_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&config_resource_id()),
            &hex32_str(&game_id),
            deposit,
            &hex32_str(&covenant_id),
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);
        assert_eq!(tx.outputs[0].value, deposit);
        let (_, ix) = decode_lane(&tx);
        assert_eq!(ix.actions.len(), 2);
        assert_eq!(ix.actions[0].action_tag, ActionTag::Deposit);
        assert_eq!(ix.actions[1].action_tag, ActionTag::JoinGame);
    }

    #[test]
    fn turn_tx_single_action() {
        let signer = test_signer();
        let net = test_net();
        let address = test_address(&signer);
        let utxo = test_utxo(&address);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let my_user_id = derive_user_resource(&lock.id_hash());
        let opponent_user_id = ResourceId::from([0x99u8; 32]);
        let game_id = ResourceId::from([0x42u8; 32]);

        let bytes = turn_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&game_id),
            &hex32_str(&my_user_id),
            &hex32_str(&opponent_user_id),
            4,
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);

        let (access, ix) = decode_lane(&tx);
        assert_eq!(access.len(), 3);
        assert!(access[0].resource_id < access[1].resource_id);
        assert!(access[1].resource_id < access[2].resource_id);
        assert_eq!(ix.actions.len(), 1);
        assert_eq!(ix.actions[0].action_tag, ActionTag::Turn);
        match &ix.actions[0].body {
            ActionBody::Turn { game_idx, user_idx, cell } => {
                assert_eq!(access[*game_idx as usize].resource_id, game_id);
                assert_eq!(access[*user_idx as usize].resource_id, my_user_id);
                assert_eq!(*cell, 4);
            }
            _ => panic!("expected turn action"),
        }
        assert_eq!(ix.signers.len(), 1);
        assert_eq!(access[ix.signers[0].0 as usize].resource_id, my_user_id);

        // Error paths construct `JsError`, which only exists on wasm32.
        #[cfg(target_arch = "wasm32")]
        assert!(
            turn_tx(
                &test_privkey_hex(),
                &net,
                &utxo,
                &address.to_string(),
                &lane_subnet_hex(),
                &hex32_str(&game_id),
                &hex32_str(&my_user_id),
                &hex32_str(&opponent_user_id),
                9,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn transfer_tx_plain_and_create() {
        let signer = test_signer();
        let net = test_net();
        let address = test_address(&signer);
        let utxo = test_utxo(&address);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let my_user_id = derive_user_resource(&lock.id_hash());
        let dest_user_id = ResourceId::from([0x55u8; 32]);
        let dest_pubkey = [0x66u8; 32];

        // Plain transfer to an existing destination.
        let bytes = transfer_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&dest_user_id),
            true,
            1_000,
            None,
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);
        let (access, ix) = decode_lane(&tx);
        assert_eq!(access.len(), 2);
        assert_eq!(ix.actions.len(), 1);
        match &ix.actions[0].body {
            ActionBody::Transfer { source_idx, dest_idx, amount, dest_init } => {
                assert_eq!(access[*source_idx as usize].resource_id, my_user_id);
                assert_eq!(access[*dest_idx as usize].resource_id, dest_user_id);
                assert_eq!(*amount, 1_000);
                assert!(dest_init.is_none());
            }
            _ => panic!("expected transfer action"),
        }

        // Creating transfer: dest lock present and derived from the dest pubkey.
        let bytes = transfer_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&dest_user_id),
            false,
            1_000,
            Some(hex32_str(&dest_pubkey)),
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);
        let (_, ix) = decode_lane(&tx);
        match &ix.actions[0].body {
            ActionBody::Transfer { dest_init, .. } => {
                let want = LockEnum::Schnorr(SchnorrLockView { pubkey: &dest_pubkey });
                assert_eq!(dest_init.expect("dest lock present").id_hash(), want.id_hash());
            }
            _ => panic!("expected transfer action"),
        }

        // Creating transfer without a dest pubkey is rejected.
        #[cfg(target_arch = "wasm32")]
        assert!(
            transfer_tx(
                &test_privkey_hex(),
                &net,
                &utxo,
                &address.to_string(),
                &lane_subnet_hex(),
                &hex32_str(&dest_user_id),
                false,
                1_000,
                None,
                None,
            )
            .is_err()
        );

        // Self-transfer is rejected: it would repeat one resource id in the access list.
        #[cfg(target_arch = "wasm32")]
        assert!(
            transfer_tx(
                &test_privkey_hex(),
                &net,
                &utxo,
                &address.to_string(),
                &lane_subnet_hex(),
                &hex32_str(&my_user_id),
                true,
                1_000,
                None,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn withdraw_tx_pays_own_pubkey() {
        let signer = test_signer();
        let net = test_net();
        let address = test_address(&signer);
        let utxo = test_utxo(&address);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let user_id = derive_user_resource(&lock.id_hash());

        let bytes = withdraw_tx(
            &test_privkey_hex(),
            &net,
            &utxo,
            &address.to_string(),
            &lane_subnet_hex(),
            &hex32_str(&config_resource_id()),
            123_456,
            None,
        )
        .unwrap();
        let tx = decode_built(&bytes);

        let (access, ix) = decode_lane(&tx);
        assert_eq!(access.len(), 2);
        assert_eq!(ix.actions.len(), 1);
        match &ix.actions[0].body {
            ActionBody::Withdraw { user_idx, config_idx, amount, dest } => {
                assert_eq!(access[*user_idx as usize].resource_id, user_id);
                assert_eq!(access[*config_idx as usize].resource_id, config_resource_id());
                assert_eq!(*amount, 123_456);
                assert_eq!(*dest, StandardSpk::PubKey(&signer.pubkey()));
            }
            _ => panic!("expected withdraw action"),
        }
        assert_eq!(ix.signers.len(), 1);
        assert_eq!(access[ix.signers[0].0 as usize].resource_id, user_id);
    }

    #[test]
    fn target_feerate_prices_above_the_floor() {
        let signer = test_signer();
        let net = test_net();
        let address = test_address(&signer);
        let utxo = test_utxo(&address);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &signer.pubkey() });
        let my_user_id = derive_user_resource(&lock.id_hash());
        let opponent_user_id = ResourceId::from([0x99u8; 32]);
        let game_id = ResourceId::from([0x42u8; 32]);

        // Same carrier over the same funding UTXO: the fee is the funded amount minus the
        // outputs, so a target-priced fee strictly above the floor-priced one is observable
        // in the change value alone.
        let fee = |feerate: Option<f64>| {
            let bytes = turn_tx(
                &test_privkey_hex(),
                &net,
                &utxo,
                &address.to_string(),
                &lane_subnet_hex(),
                &hex32_str(&game_id),
                &hex32_str(&my_user_id),
                &hex32_str(&opponent_user_id),
                4,
                feerate,
            )
            .unwrap();
            let tx = decode_built(&bytes);
            utxo.amount - tx.outputs.iter().map(|o| o.value).sum::<u64>()
        };

        let floor = fee(None);
        let target = fee(Some(1_000.0));
        assert!(target > floor, "target fee {target} must exceed the floor fee {floor}");
    }

    /// A no-op seq-commit accessor for engine runs against fabricated chains.
    struct NullAccessor;
    impl SeqCommitAccessor for NullAccessor {
        fn is_chain_ancestor_from_pov(&self, _: Hash) -> Option<bool> {
            None
        }
        fn seq_commitment_within_depth(&self, _: Hash) -> Option<Hash> {
            None
        }
    }

    /// Executes one input's scripts against the tx, mirroring the claim-kit tests.
    fn run_input(tx: &Transaction, utxos: &[UtxoEntry], idx: usize) -> Result<(), String> {
        let sig_cache = Cache::new(10_000);
        let reused = SigHashReusedValuesUnsync::new();
        let flags = EngineFlags::default();
        let populated = PopulatedTransaction::new(tx, utxos.to_vec());
        let cov_ctx =
            CovenantsContext::from_tx(&populated).expect("covenant continuity must succeed");
        let accessor = NullAccessor;
        let exec_ctx = EngineContext::new(&sig_cache)
            .with_reused(&reused)
            .with_seq_commit_accessor(&accessor)
            .with_covenants_ctx(&cov_ctx);
        let mut vm = TxScriptEngine::from_transaction_input(
            &populated,
            &tx.inputs[idx],
            idx,
            &utxos[idx],
            exec_ctx,
            flags,
        );
        vm.execute().map_err(|e| format!("{e:?}"))
    }

    /// Executes the permission input and the collateral input: the permission script enforces
    /// the spend shape, the collateral input's P2PK sig commits to exactly the submitted
    /// outputs.
    fn run_permission_input(tx: &Transaction, utxos: &[UtxoEntry]) -> Result<(), String> {
        run_input(tx, utxos, 0)?;
        run_input(tx, utxos, tx.inputs.len() - 1)
    }

    /// Decodes a hex string (test-local mirror of the ops helper).
    fn hex_bytes(s: &str) -> Vec<u8> {
        let mut out = vec![0u8; s.len() / 2];
        faster_hex::hex_decode(s.as_bytes(), &mut out).expect("valid hex");
        out
    }

    /// The claimer's collateral UTXO at the test key: a plain schnorr P2PK.
    fn collateral_utxo() -> UtxoCandidate {
        let spk = pay_to_address_script(&test_address(&test_signer()));
        UtxoCandidate {
            txid_hex: Hash::from_bytes([3u8; 32]).to_string(),
            index: 0,
            amount: 1_000_000_000,
            spk_hex: faster_hex::hex_string(spk.script()),
            spk_version: spk.version(),
        }
    }

    /// Rebuilds the UTXO entries the claim builder's transaction spends.
    fn claim_utxos(
        covenant_id: &[u8; 32],
        old_root: &[u8; 32],
        old_unclaimed: u64,
        depth: usize,
        rent: u64,
        delegates: &[UtxoCandidate],
        collateral: &UtxoCandidate,
    ) -> Vec<UtxoEntry> {
        let perm_spk = pay_to_script_hash_script(&build_permission_redeem_script(
            old_root,
            old_unclaimed,
            depth,
        ));
        let mut utxos =
            vec![UtxoEntry::new(rent, perm_spk, 0, false, Some(Hash::from_bytes(*covenant_id)))];
        let delegate_spk = pay_to_script_hash_script(&build_delegate_entry_script(covenant_id));
        for d in delegates {
            utxos.push(UtxoEntry::new(d.amount, delegate_spk.clone(), 0, false, None));
        }
        let spk =
            ScriptPublicKey::new(collateral.spk_version, hex_bytes(&collateral.spk_hex).into());
        utxos.push(UtxoEntry::new(collateral.amount, spk, 0, false, None));
        utxos
    }

    #[test]
    fn claim_tx_passes_script_engine() {
        let my_pk = [0x31u8; 32];
        let leaves = vec![
            ExitLeaf::from_pair(StandardSpk::PubKey(&my_pk), 5_000),
            ExitLeaf::from_pair(StandardSpk::PubKey(&[0x32u8; 32]), 6_000),
        ];
        let tree = PermissionTreeView::from_leaves(&leaves);
        let covenant_id = [0xFFu8; 32];
        // Overfunded delegate pool, so the claim's UTXO footprint genuinely expands.
        let delegate = UtxoCandidate {
            txid_hex: Hash::from_bytes([2u8; 32]).to_string(),
            index: 1,
            amount: 8_000,
            spk_hex: String::new(),
            spk_version: 0,
        };
        let collateral = collateral_utxo();

        let bytes = claim_tx(
            &test_privkey_hex(),
            &hex32_str(&covenant_id),
            &Hash::from_bytes([1u8; 32]).to_string(),
            0,
            50_000_000,
            &hex32_str(&tree.root()),
            2,
            tree.depth() as u32,
            0,
            &faster_hex::hex_string(leaves[0].script_bytes()),
            5_000,
            &hex32_str(&tree.root_with_leaf(0, PermissionTreeAccumulator::hash_empty())),
            1,
            claim_siblings(&leaves, 0).iter().map(|s| faster_hex::hex_string(s)).collect(),
            vec![delegate.clone()],
            collateral.clone(),
            0,
        )
        .unwrap();
        let tx = decode_built(&bytes);

        // A continuation output expands the UTXO footprint, so the KIP-0009 storage-mass
        // commitment must be present and non-zero.
        assert!(tx.storage_mass() > 0, "non-terminal claim must commit storage mass");

        let utxos = claim_utxos(
            &covenant_id,
            &tree.root(),
            2,
            tree.depth(),
            50_000_000,
            &[delegate],
            &collateral,
        );
        // The committed mass must equal the consensus calculator over the real covenant-aware
        // entries (the permission input and continuation output count plurality 2).
        let calc = MassCalculator::new(
            SIMNET_PARAMS.mass_per_tx_byte,
            SIMNET_PARAMS.mass_per_script_pub_key_byte,
            SIMNET_PARAMS.storage_mass_parameter,
        );
        let masses = calc
            .calc_contextual_masses(&PopulatedTransaction::new(&tx, utxos.clone()))
            .expect("contextual mass over claim entries");
        assert_eq!(
            tx.storage_mass(),
            masses.storage_mass,
            "committed storage mass must match the consensus calculator"
        );
        run_permission_input(&tx, &utxos).expect("full-leaf claim spend verifies");
    }

    #[test]
    fn claim_tx_fee_reduces_collateral_change() {
        let my_pk = [0x31u8; 32];
        let leaves = vec![
            ExitLeaf::from_pair(StandardSpk::PubKey(&my_pk), 5_000),
            ExitLeaf::from_pair(StandardSpk::PubKey(&[0x32u8; 32]), 6_000),
        ];
        let tree = PermissionTreeView::from_leaves(&leaves);
        let covenant_id = [0xFFu8; 32];
        let delegate = UtxoCandidate {
            txid_hex: Hash::from_bytes([2u8; 32]).to_string(),
            index: 1,
            amount: 8_000,
            spk_hex: String::new(),
            spk_version: 0,
        };
        let collateral = collateral_utxo();

        let bytes = claim_tx(
            &test_privkey_hex(),
            &hex32_str(&covenant_id),
            &Hash::from_bytes([1u8; 32]).to_string(),
            0,
            50_000_000,
            &hex32_str(&tree.root()),
            2,
            tree.depth() as u32,
            0,
            &faster_hex::hex_string(leaves[0].script_bytes()),
            5_000,
            &hex32_str(&tree.root_with_leaf(0, PermissionTreeAccumulator::hash_empty())),
            1,
            claim_siblings(&leaves, 0).iter().map(|s| faster_hex::hex_string(s)).collect(),
            vec![delegate.clone()],
            collateral.clone(),
            1_000,
        )
        .unwrap();
        let tx = decode_built(&bytes);
        // Payout, permission continuation, the exact delegate change, and the collateral
        // change minus the fee.
        assert_eq!(tx.outputs[0].value, 5_000);
        assert_eq!(tx.outputs[1].value, 50_000_000);
        assert_eq!(tx.outputs[2].value, 3_000);
        assert_eq!(tx.outputs[3].value, collateral.amount - 1_000);

        let utxos = claim_utxos(
            &covenant_id,
            &tree.root(),
            2,
            tree.depth(),
            50_000_000,
            &[delegate],
            &collateral,
        );
        run_permission_input(&tx, &utxos).expect("fee-bearing claim spend verifies");

        // A fee not strictly below the collateral is rejected (zero change is network dust).
        #[cfg(target_arch = "wasm32")]
        {
            let lean = UtxoCandidate { amount: 1_000, ..collateral };
            assert!(
                claim_tx(
                    &test_privkey_hex(),
                    &hex32_str(&covenant_id),
                    &Hash::from_bytes([1u8; 32]).to_string(),
                    0,
                    50_000_000,
                    &hex32_str(&tree.root()),
                    2,
                    tree.depth() as u32,
                    0,
                    &faster_hex::hex_string(leaves[0].script_bytes()),
                    5_000,
                    &hex32_str(&tree.root_with_leaf(0, PermissionTreeAccumulator::hash_empty())),
                    1,
                    claim_siblings(&leaves, 0).iter().map(|s| faster_hex::hex_string(s)).collect(),
                    vec![],
                    lean,
                    1_000,
                )
                .is_err()
            );
        }
    }
}
