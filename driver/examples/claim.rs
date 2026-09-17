//! `claim`: standalone L1 claimer for a ttflow run's exit leaf.
//!
//! Reads the settled exit feed from the DA server (`GET /api/exits`), finds the first unclaimed
//! leaf paying player A's pubkey (the fixed `TTFLOW_PLAYER_A_KEY` the scenario pinned), builds
//! the full-leaf claim through the web encoder's `claim_tx` builder (probe fee 0, then the
//! node's feerate-priced fee burned from the claimer's own collateral UTXO), submits it through
//! the ordinary mempool path, and waits for the payout.

use std::{str::FromStr, sync::Arc, time::Duration};

use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::tx::{ScriptPublicKey, Transaction, TransactionOutpoint};
use kaspa_hashes::Hash;
use kaspa_rpc_core::{RpcTransaction, api::rpc::RpcApi};
use kaspa_wrpc_client::prelude::*;
use secp256k1::SecretKey;
use vprog_tictactoe_driver::config::parse_network;
use vprogs_zk_abi::withdrawal::StandardSpk;
use vprogs_zk_backend_risc0_api::delegate_entry_spk_hash;
use vprogs_zk_backend_risc0_app_kit::Bip340Signer;

/// The served exit-view fields a full-leaf claim needs, parsed from `GET /api/exits`.
struct ClaimView {
    /// Raw root of the exit family (the redeem's `old_root` space).
    root: String,
    /// Settlement txid holding the permission output.
    settlement_txid: String,
    /// Index of the permission output in the settlement tx.
    outpoint_index: u32,
    /// Rent held by the permission output.
    rent: u64,
    /// Leaves of this root still unclaimed.
    unclaimed: u64,
    /// Position of the matched leaf in the root's leaf list.
    leaf_index: u32,
    /// Matched leaf's script-public-key hex.
    leaf_spk_hex: String,
    /// Matched leaf's value in sompi.
    leaf_amount: u64,
    /// Root after a full-leaf deduct of the matched leaf.
    new_root: String,
    /// Unclaimed count after a full-leaf deduct.
    new_unclaimed: u64,
    /// Sibling hashes from the matched leaf to the root.
    siblings: Vec<String>,
}

/// The claimer's fee collateral: the largest own UTXO at the payout address plus the signing key.
struct Collateral {
    /// The funding outpoint.
    outpoint: TransactionOutpoint,
    /// The UTXO's value in sompi.
    amount: u64,
    /// The UTXO's script public key (a schnorr P2PK paying player A's key).
    spk: ScriptPublicKey,
    /// Player A's private key, signing the collateral input.
    secret: [u8; 32],
}

/// Picks the claimer's largest UTXO at `address` as the fee collateral; `None` when the address
/// owns nothing.
async fn own_collateral<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    address: &Address,
    secret: [u8; 32],
) -> Option<Collateral> {
    let entries =
        client.get_utxos_by_addresses(vec![address.clone()]).await.expect("fetch collateral utxos");
    let pick = entries.iter().max_by_key(|e| e.utxo_entry.amount)?;
    Some(Collateral {
        outpoint: TransactionOutpoint::from(pick.outpoint),
        amount: pick.utxo_entry.amount,
        spk: pick.utxo_entry.script_public_key.clone(),
        secret,
    })
}

/// The claim fee from the node's feerate estimation (mirrors the web `claimFee`): the priority
/// bucket's feerate (sompi/gram) times the tx byte length plus one sigop compute mass (~10k
/// grams; byte length alone under-prices once the collateral P2PK signature is added). The
/// fee never changes the byte length, so the probe is built at fee 0.
async fn claim_fee<C: RpcApi + ?Sized>(client: &Arc<C>, probe: &Transaction) -> u64 {
    let estimate = client.get_fee_estimate().await.expect("fee estimate");
    let feerate = estimate.priority_bucket.feerate;
    let grams = borsh::to_vec(&probe).expect("serialize probe").len() as f64 + 10_000.0;
    (feerate * grams).ceil() as u64
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Finds the first root and unclaimed leaf whose script pays `target_spk_hex`, gathering every
/// served field the encoder's `claim_tx` needs. Depth rides the served siblings (one per level,
/// the same derivation the web frontend uses).
fn find_claimable(feed: &serde_json::Value, target_spk_hex: &str) -> Option<ClaimView> {
    for root in feed["roots"].as_array()? {
        for leaf in root["leaves"].as_array()? {
            if !leaf["spent"].is_null() {
                continue;
            }
            if leaf["spk_hex"].as_str()?.eq_ignore_ascii_case(target_spk_hex) {
                return Some(ClaimView {
                    root: root["root"].as_str()?.to_string(),
                    settlement_txid: root["settlement_txid"].as_str()?.to_string(),
                    outpoint_index: root["outpoint_index"].as_u64()? as u32,
                    rent: root["rent"].as_u64()?,
                    unclaimed: root["unclaimed"].as_u64()?,
                    leaf_index: leaf["index"].as_u64()? as u32,
                    leaf_spk_hex: leaf["spk_hex"].as_str()?.to_string(),
                    leaf_amount: leaf["amount"].as_u64()?,
                    new_root: leaf["full_claim"]["new_root"].as_str()?.to_string(),
                    new_unclaimed: leaf["full_claim"]["new_unclaimed"].as_u64()?,
                    siblings: leaf["siblings"]
                        .as_array()?
                        .iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect(),
                });
            }
        }
    }
    None
}

/// Connects a Borsh wRPC client to the target node.
async fn connect_wrpc(
    url: &str,
    network_id: kaspa_consensus_core::network::NetworkId,
) -> Arc<KaspaRpcClient> {
    let client =
        KaspaRpcClient::new_with_args(WrpcEncoding::Borsh, Some(url), None, Some(network_id), None)
            .expect("create wRPC client");
    client
        .connect(Some(ConnectOptions {
            block_async_connect: true,
            connect_timeout: Some(Duration::from_millis(10_000)),
            ..Default::default()
        }))
        .await
        .expect("connect to node wRPC");
    Arc::new(client)
}

/// Reads a required non-empty environment variable, panicking when missing.
fn req_env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| panic!("missing required env var {key}"))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    kaspa_core::log::try_init_logger("info");

    let wrpc_url = req_env("TT_WRPC_URL");
    let network_id = parse_network(&std::env::var("TT_NETWORK").unwrap_or_else(|_| "tn10".into()));
    let covenant_id = Hash::from_str(req_env("TT_COVENANT_ID").trim())
        .expect("TT_COVENANT_ID must be 32-byte hex");
    let da_url = std::env::var("TT_DA_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:9880".into());
    let player_a_secret = SecretKey::from_str(req_env("TTFLOW_PLAYER_A_KEY").trim())
        .expect("TTFLOW_PLAYER_A_KEY must be a 32-byte hex secp256k1 key");

    let prefix = Prefix::from(network_id);
    let player_a = Bip340Signer::from_secret_key(&player_a_secret);
    let pubkey = player_a.pubkey();
    let payout_address = Address::new(prefix, Version::PubKey, &pubkey);
    println!(
        "player A payout address (claim collateral and leaf payouts land here): {payout_address}"
    );

    let client = connect_wrpc(&wrpc_url, network_id).await;
    log::info!("connected to {wrpc_url}");

    // Served exit feed: the first unclaimed leaf paying player A's pubkey script.
    let target_spk_hex = hex(StandardSpk::PubKey(&pubkey).to_script_bytes().as_slice());
    let feed: serde_json::Value = reqwest::get(format!("{da_url}/api/exits"))
        .await
        .expect("fetch /api/exits")
        .json()
        .await
        .expect("decode /api/exits");
    let Some(view) = find_claimable(&feed, &target_spk_hex) else {
        eprintln!("no unclaimed exit leaf pays player A at {da_url}/api/exits");
        std::process::exit(1);
    };

    // Delegates fund the payout from the covenant deposit pool; they are conserved exact.
    let deposit_address = Address::new(
        prefix,
        Version::ScriptHash,
        &delegate_entry_spk_hash(&covenant_id.as_bytes()),
    );
    let delegate_entries = client
        .get_utxos_by_addresses(vec![deposit_address.clone()])
        .await
        .expect("fetch delegate utxos");
    let delegates: Vec<(TransactionOutpoint, u64)> = delegate_entries
        .into_iter()
        .map(|e| (TransactionOutpoint::from(e.outpoint), e.utxo_entry.amount))
        .collect();
    let delegate_total: u64 = delegates.iter().map(|(_, amount)| *amount).sum();
    if delegate_total < view.leaf_amount {
        eprintln!(
            "delegate pool {delegate_total} at {deposit_address} cannot cover the {}-sompi leaf",
            view.leaf_amount
        );
        std::process::exit(1);
    }

    // The claim fee burns from the claimer's own collateral at the payout address.
    let Some(collateral) =
        own_collateral(&client, &payout_address, player_a_secret.secret_bytes()).await
    else {
        println!("payout address {payout_address} has no UTXOs");
        println!("fund this address with collateral first (the claim fee burns from it)");
        std::process::exit(2);
    };

    // Build through the web encoder's `claim_tx` exactly like the frontend: probe at fee 0,
    // price from the node's feerate estimation, rebuild with the real fee.
    let delegate_utxos = delegates
        .into_iter()
        .map(|(outpoint, amount)| vprog_tictactoe_encoder_wasm::UtxoCandidate {
            txid_hex: outpoint.transaction_id.to_string(),
            index: outpoint.index,
            amount,
            spk_hex: String::new(),
            spk_version: 0,
        })
        .collect::<Vec<_>>();
    let collateral_utxo = vprog_tictactoe_encoder_wasm::UtxoCandidate {
        txid_hex: collateral.outpoint.transaction_id.to_string(),
        index: collateral.outpoint.index,
        amount: collateral.amount,
        spk_hex: hex(collateral.spk.script()),
        spk_version: collateral.spk.version(),
    };
    let privkey_hex = hex(&collateral.secret);
    let covenant_id_hex = covenant_id.to_string();
    let build = |fee: u64| {
        vprog_tictactoe_encoder_wasm::claim_tx(
            &privkey_hex,
            &covenant_id_hex,
            &view.settlement_txid,
            view.outpoint_index,
            view.rent,
            &view.root,
            view.unclaimed,
            view.siblings.len() as u32,
            view.leaf_index,
            &view.leaf_spk_hex,
            view.leaf_amount,
            &view.new_root,
            view.new_unclaimed,
            view.siblings.clone(),
            delegate_utxos.clone(),
            collateral_utxo.clone(),
            fee,
        )
    };
    let probe = build(0).expect("encoder probe claim assembly failed");
    let probe_tx: Transaction = borsh::from_slice(&probe).expect("decode encoder probe claim");
    let fee = claim_fee(&client, &probe_tx).await;
    let bytes = build(fee).expect("encoder claim assembly failed");
    let claim_tx: Transaction = borsh::from_slice(&bytes).expect("decode encoder claim");
    let claim_txid = claim_tx.id();

    let submitted = client
        .submit_transaction(RpcTransaction::from(&claim_tx), false)
        .await
        .expect("fee-bearing claim must enter the mempool");
    assert_eq!(submitted, claim_txid, "the node must accept the claim under its own txid");

    // Wait for the payout: on the last leaf of a root the builder folds the permission rent
    // into the payout (the same expected value the e2e asserts).
    let expected_payout = view.leaf_amount + if view.unclaimed == 1 { view.rent } else { 0 };
    let timeout = Duration::from_secs(600);
    let start = tokio::time::Instant::now();
    loop {
        let entries = client
            .get_utxos_by_addresses(vec![payout_address.clone()])
            .await
            .expect("fetch payout utxos");
        if entries.iter().any(|e| {
            TransactionOutpoint::from(e.outpoint).transaction_id == claim_txid
                && e.utxo_entry.amount == expected_payout
        }) {
            println!("== claim complete ==");
            println!("Claim tx:         {claim_txid}");
            println!("Paid:             {expected_payout} sompi at {payout_address}");
            return;
        }
        if start.elapsed() > timeout {
            eprintln!("timed out waiting for the {expected_payout}-sompi payout from {claim_txid}");
            std::process::exit(1);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
