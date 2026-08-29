//! `ttflow`: scripted tic-tac-toe scenario driver.
//!
//! Issues signed action carrier transactions (config init, deposits, game lifecycle, moves, and
//! winner withdrawal) for flow tests and live demos.

mod config;
pub mod scenario;

use std::{sync::Arc, time::Duration};

use config::Config;
use kaspa_consensus_core::{config::params::Params, network::NetworkId};
use kaspa_wrpc_client::prelude::*;

/// Connects a Borsh wRPC client to the target node.
async fn connect_wrpc(url: &str, network_id: NetworkId) -> Arc<KaspaRpcClient> {
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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    kaspa_core::log::try_init_logger(
        "info,vprog_tictactoe_driver=info,vprogs_l1_wallet=info,vprogs_zk_backend_risc0_app_kit=info",
    );

    let cfg = Config::from_env();
    let params = Params::from(cfg.network_id);
    let client = connect_wrpc(&cfg.wrpc_url, cfg.network_id).await;
    log::info!("connected to {}", cfg.wrpc_url);

    println!("== ttflow scenario driver: lane={} ==", cfg.lane_id);
    match scenario::run(&client, &params, &cfg).await {
        Ok(report) => {
            println!("== ttflow scenario complete ==");
            println!("Init tx:          {}", report.init_txid);
            println!("Deposit A tx:     {}", report.deposit_a_txid);
            println!("Deposit B tx:     {}", report.deposit_b_txid);
            println!("CreateGame tx:    {}", report.create_game_txid);
            println!("JoinGame tx:      {}", report.join_game_txid);
            println!("Turn A tx:        {}", report.turn_a_txid);
            println!("Turn B tx:        {}", report.turn_b_txid);
            println!("Withdraw tx:      {}", report.withdraw_txid);
        }
        Err(e) => {
            log::error!("scenario failed: {e}");
            std::process::exit(1);
        }
    }
}
