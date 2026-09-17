//! `wrpc-miner`: solo CPU miner speaking plain wRPC to a full node, no local kaspad needed.
//!
//! Loops `get_block_template` (coinbase pays the given address), grinds nonces through
//! kaspa-pow's precomputed [`State`], and submits winners with `submit_block`. Refreshes the
//! template every couple of seconds so the mined block rides a fresh tip. Usage:
//! `wrpc-miner <url> <pay-address>`; network comes from `TT_NETWORK` (default `tn10`).

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use kaspa_consensus_core::header::Header;
use kaspa_wrpc_client::prelude::*;

/// How long to grind one template before fetching a fresh one.
const TEMPLATE_REFRESH: Duration = Duration::from_secs(2);

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    kaspa_core::log::try_init_logger("info");

    let mut args = std::env::args().skip(1);
    let url = args.next().expect("usage: wrpc-miner <url> <pay-address>");
    let pay_address = kaspa_addresses::Address::try_from(
        args.next().expect("usage: wrpc-miner <url> <pay-address>").as_str(),
    )
    .expect("valid pay address");
    let network_id = vprog_tictactoe_driver::config::parse_network(
        &std::env::var("TT_NETWORK").unwrap_or_else(|_| "tn10".into()),
    );

    let client: Arc<KaspaRpcClient> = Arc::new(
        KaspaRpcClient::new_with_args(
            WrpcEncoding::Borsh,
            Some(&url),
            None,
            Some(network_id),
            None,
        )
        .expect("create wrpc client"),
    );
    client
        .connect(Some(ConnectOptions {
            block_async_connect: true,
            connect_timeout: Some(Duration::from_millis(10_000)),
            ..Default::default()
        }))
        .await
        .expect("connect to node");
    log::info!("mining {pay_address} via {url}");

    let extra_data = b"tictactoe-demo".to_vec();
    let mut found = 0u64;
    let mut attempted = 0u64;
    let started = Instant::now();
    loop {
        let template =
            match client.get_block_template(pay_address.clone(), extra_data.clone()).await {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("template fetch failed: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
        let header: Header = (&template.block.header).try_into().expect("header decode");
        let state = kaspa_pow::State::new(&header);
        let deadline = Instant::now() + TEMPLATE_REFRESH;
        let mut nonce = 0u64;
        'grind: while Instant::now() < deadline {
            // A small nonce batch per clock check: check_pow is the hot loop.
            for _ in 0..4096 {
                if state.check_pow(nonce).0 {
                    let mut block = template.block.clone();
                    block.header.nonce = nonce;
                    match client.submit_block(block, false).await {
                        Ok(_) => {
                            found += 1;
                            let rate = attempted as f64 / started.elapsed().as_secs_f64();
                            log::info!("block found! nonce {nonce} (#{found}, ~{rate:.0} H/s)");
                        }
                        Err(e) => log::warn!("submit rejected (stale tip?): {e}"),
                    }
                    break 'grind;
                }
                nonce += 1;
                attempted += 1;
            }
        }
    }
}
