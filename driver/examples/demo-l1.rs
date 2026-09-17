//! `demo-l1`: standalone demo L1 for the web frontend.
//!
//! Runs an in-process simnet kaspad on fixed ports (the demo `ttd` and web frontend point at
//! ws://127.0.0.1:17210), mines a block roughly every second forever, and serves a small HTTP
//! API on 127.0.0.1:9890:
//!
//! - `GET /faucet/{address}`: pays 10 KAS from the miner's coinbase wallet to `address` and queues
//!   the payout for the next block.
//! - `POST /inject`: the request body is raw borsh-serialized [`Transaction`] bytes, mined directly
//!   into the next block. Claim txs are zero-fee by protocol, and the mempool relay floor rejects
//!   zero-fee txs, so they cannot enter the mempool and must be injected here.
//!
//! The daemon runs on a throwaway appdir, so every run starts from a fresh chain (intentional
//! for a demo). Runs until killed.

use std::{collections::HashSet, io::Write, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use borsh::BorshDeserialize;
use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::{
    Hash,
    config::params::{OverrideParams, Params},
    header::Header,
    merkle::calc_hash_merkle_root,
    network::{NetworkId, NetworkType},
    tx::{Transaction, TransactionOutpoint},
};
use kaspa_grpc_client::GrpcClient;
use kaspa_rpc_core::{RpcTransaction, api::rpc::RpcApi, error::RpcError};
use kaspa_testing_integration::common::daemon::{ClientManager, Daemon};
use kaspad_lib::args::Args;
use secp256k1::{Keypair, SECP256K1};
use vprogs_l1_wallet::{
    Wallet,
    build::{PayToAddressTx, pay_to_address_transaction},
};

/// Fixed gRPC port of the in-process daemon.
const RPC_PORT: u16 = 16110;
/// Fixed p2p port of the in-process daemon.
const P2P_PORT: u16 = 17111;
/// Fixed JSON wRPC port of the in-process daemon.
const RPC_JSON_PORT: u16 = 17310;
/// Fixed borsh wRPC port; the demo `ttd` and web frontend connect here.
const WRPC_BORSH_PORT: u16 = 17210;
/// Port of the faucet/inject HTTP API.
const HTTP_PORT: u16 = 9890;
/// Faucet payout per request, in sompi (10 KAS).
const FAUCET_PAYOUT_SOMPI: u64 = 10 * 100_000_000;
/// Blocks before a coinbase output becomes spendable (matches the daemon's override file).
const COINBASE_MATURITY: u64 = 10;
/// Default block interval in milliseconds; override with `TT_DEMO_L1_INTERVAL_MS`.
const DEFAULT_INTERVAL_MS: u64 = 1000;

/// Shared state: the daemon's gRPC client, the coinbase wallet material, and the queue of txs
/// the miner folds into the next block template.
struct DemoL1 {
    /// gRPC client for mining calls.
    grpc: GrpcClient,
    /// Consensus parameters matching the daemon's overrides.
    params: Params,
    /// Coinbase keypair collecting mining rewards and funding the faucet.
    keypair: Keypair,
    /// Coinbase address the block templates pay to.
    coinbase_address: Address,
    /// Outpoints already spent by queued payouts. The utxoindex lags the queue by a block, so
    /// back-to-back faucet calls would otherwise rebuild the same tx (a double-spend the miner
    /// drops); filtering these outpoints keeps every queued payout spendable.
    spent_outpoints: std::sync::Mutex<HashSet<TransactionOutpoint>>,
    /// Txs waiting for the next mined block.
    queue: std::sync::Mutex<Vec<Transaction>>,
}

/// Mines one block, folding `txs` directly into the template: get_block_template, inject,
/// recompute the hash merkle root, submit_block. Returns the mined block hash.
async fn mine_block(state: &DemoL1, txs: &[Transaction]) -> Result<Hash, RpcError> {
    let mut template =
        state.grpc.get_block_template(state.coinbase_address.clone(), vec![]).await?;

    if !txs.is_empty() {
        for tx in txs {
            template.block.transactions.push(RpcTransaction::from(tx));
        }

        // Recompute the hash merkle root to cover the added transactions.
        let consensus_txs: Vec<Transaction> = template
            .block
            .transactions
            .iter()
            .map(|rpc_tx| Transaction::try_from(rpc_tx.clone()).unwrap())
            .collect();
        template.block.header.hash_merkle_root = calc_hash_merkle_root(consensus_txs.iter());
    }

    let header: Header = (&template.block.header).try_into().unwrap();
    let hash = header.hash;
    state.grpc.submit_block(template.block, false).await?;
    Ok(hash)
}

/// Mines forever, one block per interval, draining the queue into each template. A failed round
/// is logged and its queued txs dropped: the demo keeps mining rather than retrying dead txs.
async fn mine_forever(state: Arc<DemoL1>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        let txs = std::mem::take(&mut *state.queue.lock().unwrap());
        match mine_block(&state, &txs).await {
            Ok(hash) => log::info!("mined {} with {} injected txs", hash, txs.len()),
            Err(e) => log::error!("mining round with {} txs failed: {e}", txs.len()),
        }
    }
}

/// `GET /faucet/{address}`: pays 10 KAS to the address from the coinbase wallet. Returns the
/// payout txid once the tx is queued for the next block. Until coinbase rewards mature (about
/// `COINBASE_MATURITY` blocks in), returns 503.
async fn faucet(
    State(state): State<Arc<DemoL1>>,
    Path(address): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let address = Address::try_from(address.as_str())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid address: {e}")))?;

    let wallet = Wallet::new(&state.grpc, &state.params, state.keypair);
    // The fallible pieces of Wallet::pay_to_address: its own expects would take the whole
    // demo down on a payout the immature coinbase cannot fund yet.
    let utxos = wallet
        .fetch_spendable_utxos()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("fetching spendable UTXOs: {e}")))?;
    let mut spent = state.spent_outpoints.lock().unwrap();
    let candidates: Vec<_> = utxos.into_iter().filter(|u| !spent.contains(&u.0)).collect();
    let tx = pay_to_address_transaction(PayToAddressTx {
        candidates,
        recipient: &address,
        value: FAUCET_PAYOUT_SOMPI,
        count: 1,
        keypair: state.keypair,
        change_address: wallet.address(),
        params: &state.params,
    })
    .map_err(|e| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "payout not fundable yet ({e}); coinbase matures after {COINBASE_MATURITY} blocks"
            ),
        )
    })?;

    let txid = tx.id();
    for input in &tx.inputs {
        spent.insert(input.previous_outpoint);
    }
    drop(spent);
    state.queue.lock().unwrap().push(tx);
    Ok(txid.to_string())
}

/// `POST /inject`: the raw request body is borsh-serialized [`Transaction`] bytes, queued for
/// the next mined block. Zero-fee claim txs are rejected by the mempool relay floor, so the web
/// demo posts them here instead of submitting them to the node.
async fn inject(
    State(state): State<Arc<DemoL1>>,
    body: Bytes,
) -> Result<String, (StatusCode, String)> {
    let tx = Transaction::try_from_slice(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid borsh transaction: {e}")))?;
    let txid = tx.id();
    state.queue.lock().unwrap().push(tx);
    Ok(txid.to_string())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    kaspa_core::log::try_init_logger("info");

    let interval_ms: u64 = std::env::var("TT_DEMO_L1_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_INTERVAL_MS);

    // Simnet defaults with fast coinbase maturity so the faucet has spendable funds seconds
    // in. The same params go to the override file the daemon reads at startup.
    let mut params = Params::from(NetworkId::new(NetworkType::Simnet));
    params.blockrate.coinbase_maturity = COINBASE_MATURITY;
    let mut params_file = tempfile::NamedTempFile::new().expect("create override-params tempfile");
    write!(
        params_file,
        "{}",
        serde_json::to_string(&OverrideParams::from(params.clone()))
            .expect("OverrideParams must serialize")
    )
    .expect("write override-params tempfile");

    let args = Args {
        simnet: true,
        unsafe_rpc: true,
        enable_unsynced_mining: true,
        disable_upnp: true,
        utxoindex: true,
        rpclisten: Some(format!("127.0.0.1:{RPC_PORT}").parse().unwrap()),
        listen: Some(format!("127.0.0.1:{P2P_PORT}").parse().unwrap()),
        rpclisten_json: Some(format!("127.0.0.1:{RPC_JSON_PORT}").parse().unwrap()),
        rpclisten_borsh: Some(format!("127.0.0.1:{WRPC_BORSH_PORT}").parse().unwrap()),
        override_params_file: Some(
            params_file.path().to_str().expect("tempfile path must be utf-8").to_string(),
        ),
        ..Default::default()
    };

    // The daemon owns a throwaway appdir, so every run starts from a fresh chain.
    let mut daemon = Daemon::with_manager(Arc::new(ClientManager::new(args)), 10);
    let grpc = daemon.start().await;

    let keypair = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
    let (xonly, _) = keypair.x_only_public_key();
    let coinbase_address =
        Address::new(Prefix::from(NetworkType::Simnet), Version::PubKey, &xonly.serialize());

    let state = Arc::new(DemoL1 {
        grpc,
        params,
        keypair,
        coinbase_address,
        spent_outpoints: Default::default(),
        queue: Default::default(),
    });

    tokio::spawn(mine_forever(state.clone(), Duration::from_millis(interval_ms)));

    let router = Router::new()
        .route("/faucet/{address}", get(faucet))
        .route("/inject", post(inject))
        .with_state(state);

    println!("== demo L1 ==");
    println!("wRPC (borsh): ws://127.0.0.1:{WRPC_BORSH_PORT}");
    println!("faucet: http://127.0.0.1:{HTTP_PORT}/faucet/<address> (pays 10 KAS)");
    println!("inject: POST http://127.0.0.1:{HTTP_PORT}/inject (raw borsh transaction bytes)");
    println!("note: claim txs are zero-fee and the mempool rejects them; mine them via /inject");

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{HTTP_PORT}")).await.unwrap();
    axum::serve(listener, router).await.unwrap();
}
