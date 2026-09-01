//! End-to-end integration test running the tic-tac-toe scenario against a local simnet L1.
//!
//! Gated behind `TT_E2E=1` and available guest program ELFs. Exercises the full thin-app stack:
//! 1. In-process simnet [`L1Node`].
//! 2. Runner execution daemon (`ttd` equivalent via [`start_runner`]).
//! 3. Scenario driver (`ttflow` equivalent via [`scenario::run`]).
//! 4. Full game lifecycle: Init -> Deposits -> CreateGame -> JoinGame -> Turns -> Settlement ->
//!    Withdraw.

use std::{sync::Arc, time::Duration};

use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::{
    config::params::ForkActivation,
    mass::BlockMassLimits,
    network::{NetworkId, NetworkType},
};
use kaspa_hashes::Hash;
use secp256k1::Keypair;
use vprog_tictactoe_driver::{config::Config, scenario};
use vprog_tictactoe_guest::runtime::genesis::GENESIS_PUBKEY;
use vprog_tictactoe_node::indexer::{
    GameStatus, PlayerEvent, TicTacToeIndexer, scan_games_by_status, scan_player_events,
};
use vprogs_node_test_utils::L1Node;
use vprogs_runner::{Elfs, Indexer, RunnerConfig, StartMode, start_runner};
use vprogs_storage_types::Store;
use vprogs_zk_backend_risc0_api::delegate_entry_spk_hash;
use vprogs_zk_backend_risc0_app_kit::dev_genesis_keypair;
use vprogs_zk_backend_risc0_test_suite::{
    batch_aggregator_elf, batch_processor_elf, dev_mode_enabled,
};

#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_simnet_game_flow() {
    // Environment gate: skipped unless explicitly requested.
    if std::env::var("TT_E2E").is_err() {
        eprintln!("skipping test_e2e_simnet_game_flow: TT_E2E is unset");
        return;
    }

    // ELF presence check: skipped cleanly when the compiled program ELF is missing.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let default_elf_path = format!("{manifest_dir}/../guest/compiled/program.elf");
    let program_elf_path = std::env::var("TT_PROGRAM_ELF").unwrap_or(default_elf_path);
    if !std::path::Path::new(&program_elf_path).exists() {
        eprintln!(
            "skipping test_e2e_simnet_game_flow: program ELF not found at {program_elf_path}"
        );
        return;
    }

    // Dev-mode assertion: integration test expects dev stub proofs on CPU.
    assert!(dev_mode_enabled(), "test_e2e_simnet_game_flow requires RISC0_DEV_MODE=1");

    // Initialize logger for test visibility.
    kaspa_core::log::try_init_logger(
        "info,vprog_tictactoe_driver=debug,vprogs_runner=debug,vprogs_node_framework=info",
    );

    // 1. Spin up simnet L1 node with raised mass limits and fast coinbase maturity.
    let network_id = NetworkId::new(NetworkType::Simnet);
    let l1 = Arc::new(
        L1Node::new(
            network_id,
            Some(|p| {
                p.blockrate.coinbase_maturity = 1;
                p.toccata_activation = ForkActivation::always();
                p.prior_block_mass_limits = BlockMassLimits::with_shared_limit(2_000_000);
            }),
        )
        .await,
    );
    l1.mine_utxos(30).await;

    // 2. Fund operator wallet address on L1.
    let operator_keypair = Keypair::new(secp256k1::SECP256K1, &mut secp256k1::rand::thread_rng());
    let operator_pubkey = operator_keypair.x_only_public_key().0.serialize();
    let operator_address = Address::new(Prefix::Simnet, Version::PubKey, &operator_pubkey);
    l1.fund_address(&operator_address, 1_000_000_000, 20).await;
    l1.mine_blocks(2).await;

    // 3. Connect wRPC client.
    let wrpc_url = l1.wrpc_borsh_url();
    let client = vprogs_runner::connect_wrpc(&wrpc_url, network_id).await;
    let arc_client = Arc::new(client);
    let params = l1.params().clone();

    // 4. Load guest program and backend ELFs.
    let program_elf_bytes = std::fs::read(&program_elf_path).expect("read program elf");
    let batch_elf_bytes = batch_processor_elf();
    let aggregator_elf_bytes = batch_aggregator_elf();
    let elfs = Elfs {
        program: &program_elf_bytes,
        batch: &batch_elf_bytes,
        aggregator: &aggregator_elf_bytes,
    };

    // 5. Start runner in execution mode with fresh covenant deployment.
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let runner_cfg = RunnerConfig {
        wrpc_url: wrpc_url.clone(),
        private_key: Some(operator_keypair.secret_key()),
        network_id,
        program_elf: None,
        batch_elf: None,
        aggregator_elf: None,
        data_dir: temp_dir.path().to_path_buf(),
        lane_id: Some(1),
        covenant_id: None,
        bootstrap_txid: None,
        start_from: None,
        seed_depth: 500,
        min_confirmations: None,
        prove: false,
        start_mode: Some(StartMode::Fresh),
    };

    let handles = start_runner(
        &runner_cfg,
        &arc_client,
        &params,
        elfs,
        delegate_entry_spk_hash,
        Some(Indexer(Arc::new(TicTacToeIndexer))),
    )
    .await
    .expect("start_runner failed");

    // Mine covenant bootstrap transaction.
    l1.mine_blocks(2).await;

    // 6. Spawn continuous block mining loop for the duration of the scenario.
    let l1_miner = l1.clone();
    let mining_stop = Arc::new(tokio::sync::Notify::new());
    let mining_stop_task = mining_stop.clone();
    let miner_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = mining_stop_task.notified() => break,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    l1_miner.mine_blocks(1).await;
                }
            }
        }
    });

    // 7. Drive the scripted scenario.
    let driver_cfg = Config {
        wrpc_url: wrpc_url.clone(),
        network_id,
        lane_id: handles.lane_id,
        covenant_id: handles.covenant_id,
        private_key: operator_keypair.secret_key(),
        genesis_key: dev_genesis_keypair(&GENESIS_PUBKEY).secret_key(),
        stake: 50_000_000,
        rounds: 1,
        deposit_amount: 100_000_000,
        step_delay: Duration::from_millis(300),
        turn_ttl: 10_000,
    };

    let report =
        scenario::run(&arc_client, &params, &driver_cfg).await.expect("scenario execution failed");

    // 8. Assert all scenario steps accepted and generated valid transaction IDs.
    assert_ne!(report.init_txid, Hash::default());
    assert_ne!(report.deposit_a_txid, Hash::default());
    assert_ne!(report.deposit_b_txid, Hash::default());
    assert_ne!(report.create_game_txid, Hash::default());
    assert_ne!(report.join_game_txid, Hash::default());
    assert_ne!(report.turn_a_txid, Hash::default());
    assert_ne!(report.turn_b_txid, Hash::default());
    assert_ne!(report.withdraw_txid, Hash::default());

    // 9. Poll until secondary index updates commit and assert canonical scans.
    let store = handles.node.api().storage().store().clone();
    let game_bytes: [u8; 32] = report.game_id.as_slice().try_into().unwrap();

    let poll_timeout = Duration::from_secs(30);
    let poll_interval = Duration::from_millis(250);
    let start_time = tokio::time::Instant::now();

    let mut indexed = false;
    while start_time.elapsed() < poll_timeout {
        let snapshot = store.canonical_chain().snapshot();

        let a_events = scan_player_events(&*store, &snapshot, &report.player_a_user_id);
        let b_events = scan_player_events(&*store, &snapshot, &report.player_b_user_id);
        let finished_games = scan_games_by_status(&*store, &snapshot, GameStatus::Finished);
        let open_games = scan_games_by_status(&*store, &snapshot, GameStatus::Open);

        let has_a_created =
            a_events.iter().any(|&(t, _, g)| t == PlayerEvent::Created && g == game_bytes);
        let has_a_won = a_events.iter().any(|&(t, _, g)| t == PlayerEvent::Won && g == game_bytes);
        let has_b_joined =
            b_events.iter().any(|&(t, _, g)| t == PlayerEvent::Joined && g == game_bytes);
        let has_b_lost =
            b_events.iter().any(|&(t, _, g)| t == PlayerEvent::Lost && g == game_bytes);
        let has_finished = finished_games.iter().any(|&(_, g)| g == game_bytes);
        let not_open = !open_games.iter().any(|&(_, g)| g == game_bytes);

        if has_a_created && has_a_won && has_b_joined && has_b_lost && has_finished && not_open {
            indexed = true;
            break;
        }

        tokio::time::sleep(poll_interval).await;
    }

    assert!(indexed, "timed out waiting for secondary index entries after completed match");

    let snapshot = store.canonical_chain().snapshot();
    let a_events = scan_player_events(&*store, &snapshot, &report.player_a_user_id);
    let b_events = scan_player_events(&*store, &snapshot, &report.player_b_user_id);
    let finished_games = scan_games_by_status(&*store, &snapshot, GameStatus::Finished);
    let open_games = scan_games_by_status(&*store, &snapshot, GameStatus::Open);

    let a_created = a_events
        .iter()
        .find(|&&(t, _, g)| t == PlayerEvent::Created && g == game_bytes)
        .expect("player A must have a Created event for game");
    assert!(snapshot.is_canonical(a_created.1), "Created version must be canonical");

    let a_won = a_events
        .iter()
        .find(|&&(t, _, g)| t == PlayerEvent::Won && g == game_bytes)
        .expect("player A must have a Won event for game");
    assert!(snapshot.is_canonical(a_won.1), "Won version must be canonical");

    let b_joined = b_events
        .iter()
        .find(|&&(t, _, g)| t == PlayerEvent::Joined && g == game_bytes)
        .expect("player B must have a Joined event for game");
    assert!(snapshot.is_canonical(b_joined.1), "Joined version must be canonical");

    let b_lost = b_events
        .iter()
        .find(|&&(t, _, g)| t == PlayerEvent::Lost && g == game_bytes)
        .expect("player B must have a Lost event for game");
    assert!(snapshot.is_canonical(b_lost.1), "Lost version must be canonical");

    let finished_entry = finished_games
        .iter()
        .find(|&&(_, g)| g == game_bytes)
        .expect("game must appear in the Finished status index");
    assert!(snapshot.is_canonical(finished_entry.0), "Finished version must be canonical");

    assert!(
        !open_games.iter().any(|&(_, g)| g == game_bytes),
        "game must not appear in the Open status index"
    );

    // Stop background miner and cleanup.
    mining_stop.notify_one();
    let _ = miner_handle.await;
    l1.mine_blocks(2).await;
}
