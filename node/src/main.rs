//! `ttd`: the vprog-tictactoe node daemon.
//!
//! Drives the vprogs runner with the tic-tac-toe guest program. Contains zero payload knowledge;
//! app-specific state transitions and rules live entirely in the guest ELF.

mod config;

use std::sync::Arc;

use config::Config;
use kaspa_consensus_core::config::params::Params;
use vprog_tictactoe_node::{TicTacToeExitIndexer, indexer::TicTacToeIndexer};
use vprogs_runner::{Indexer, connect_wrpc, start_runner};
use vprogs_zk_backend_risc0_api::delegate_entry_spk_hash;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    kaspa_core::log::try_init_logger(
        "info,vprog_tictactoe_node=info,vprogs_node_framework=trace,vprogs_zk_vm=trace,risc0_zkvm=warn",
    );

    let cfg = Config::from_env();
    let elfs = cfg.runner.load_elfs().unwrap_or_else(|e| panic!("failed to load guest ELFs: {e}"));

    let network_id = cfg.runner.network_id;
    let params = Params::from(network_id);
    let client = connect_wrpc(&cfg.runner.wrpc_url, network_id).await;
    log::info!("connected to {}", cfg.runner.wrpc_url);

    let handles = start_runner(
        &cfg.runner,
        &client,
        &params,
        elfs.as_elfs(),
        delegate_entry_spk_hash,
        Some(Indexer(Arc::new(TicTacToeIndexer))),
        Some(Arc::new(TicTacToeExitIndexer::default())),
    )
    .await
    .unwrap_or_else(|e| panic!("runner start failed: {e}"));

    let da_state = vprog_tictactoe_node::da::DaState {
        store: handles.node.api().storage().store().clone(),
        covenant_id: handles.covenant_id.as_bytes(),
        lane_subnet: handles.lane_subnet.as_bytes().to_vec(),
        network_prefix: kaspa_addresses::Prefix::from(network_id).to_string(),
        web_dir: cfg.web_dir.clone(),
    };
    let da_router = vprog_tictactoe_node::da::router(da_state);
    let listener = tokio::net::TcpListener::bind(&cfg.da_bind)
        .await
        .unwrap_or_else(|e| panic!("failed to bind DA server on {}: {e}", cfg.da_bind));
    log::info!("DA server listening on http://{}", cfg.da_bind);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, da_router).await {
            log::error!("DA server error: {e}");
        }
    });

    println!("== ttd node: lane={} ==", handles.lane_id);
    println!("watch RUST_LOG trace for vprogs_node_framework and vprogs_zk_vm");

    // Keep the node alive for the process lifetime.
    let _node = handles.node;
    match handles.settler {
        Some((handle, shutdown)) => {
            ctrlc::set_handler(move || shutdown.open()).expect("set signal handler");
            match handle.await {
                Ok(()) => log::info!("settler finished; shutting down"),
                Err(e) => {
                    log::error!("settler task terminated abnormally: {e}");
                    std::process::exit(1);
                }
            }
        }
        None => std::future::pending::<()>().await,
    }
}
