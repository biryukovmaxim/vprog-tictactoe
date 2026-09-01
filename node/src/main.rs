//! `ttd`: the vprog-tictactoe node daemon.
//!
//! Drives the vprogs runner with the tic-tac-toe guest program. Contains zero payload knowledge;
//! app-specific state transitions and rules live entirely in the guest ELF.

mod config;
mod indexer;

use config::Config;
use kaspa_consensus_core::config::params::Params;
use vprogs_runner::{connect_wrpc, start_runner};
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

    let handles =
        start_runner(&cfg.runner, &client, &params, elfs.as_elfs(), delegate_entry_spk_hash, None)
            .await
            .unwrap_or_else(|e| panic!("runner start failed: {e}"));

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
