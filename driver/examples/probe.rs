//! `probe`: times wallet-facing RPCs (`get_server_info`, `get_utxos_by_addresses`, and an
//! optional block-template fetch) against a wRPC endpoint. Usage: `probe <url> <address>
//! [network]`; exits non-zero when any call errors.

use std::{sync::Arc, time::Duration};

use kaspa_wrpc_client::prelude::*;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("usage: probe <url> <address>");
    let address = kaspa_addresses::Address::try_from(
        args.next().expect("usage: probe <url> <address>").as_str(),
    )
    .expect("valid address");
    let network_id = vprog_tictactoe_driver::config::parse_network(
        &args.next().unwrap_or_else(|| "tn10".into()),
    );

    let client = KaspaRpcClient::new_with_args(
        WrpcEncoding::Borsh,
        Some(&url),
        None,
        Some(network_id),
        None,
    )
    .expect("create wrpc client");
    let client: Arc<KaspaRpcClient> = Arc::new(client);
    client
        .connect(Some(ConnectOptions {
            block_async_connect: true,
            connect_timeout: Some(Duration::from_millis(10_000)),
            ..Default::default()
        }))
        .await
        .expect("connect");

    let t = std::time::Instant::now();
    let info = client.get_server_info().await.expect("get_server_info");
    println!(
        "get_server_info ok in {:?} (virtual_daa_score {})",
        t.elapsed(),
        info.virtual_daa_score
    );

    let t = std::time::Instant::now();
    let utxos =
        client.get_utxos_by_addresses(vec![address.clone()]).await.expect("get_utxos_by_addresses");
    println!("get_utxos_by_addresses ok in {:?} ({} entries)", t.elapsed(), utxos.len());

    // Template probe: what would solo mining pay, and at what difficulty.
    let t = std::time::Instant::now();
    let template =
        client.get_block_template(address.clone(), vec![]).await.expect("get_block_template");
    println!(
        "get_block_template ok in {:?} ({} txs)",
        t.elapsed(),
        template.block.transactions.len()
    );
    let header = &template.block.header;
    println!("header bits (compact): {:x}, daa_score {}", header.bits, header.daa_score);
}
