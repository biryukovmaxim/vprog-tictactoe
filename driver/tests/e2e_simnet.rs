//! End-to-end integration test running the tic-tac-toe scenario against a local simnet L1.
//!
//! Gated behind `TT_E2E=1` and available guest program ELFs. Exercises the full thin-app stack:
//! 1. In-process simnet [`L1Node`].
//! 2. Runner execution and settlement daemon (`ttd` equivalent via [`start_runner`]).
//! 3. Scenario driver (`ttflow` equivalent via [`scenario::run`]).
//! 4. Full game lifecycle: Init -> Deposits -> CreateGame -> JoinGame -> Turns -> Settlement ->
//!    Withdraw -> Claim -> Payout.

use std::{sync::Arc, time::Duration};

use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::{
    config::params::ForkActivation,
    mass::BlockMassLimits,
    network::{NetworkId, NetworkType},
    tx::TransactionOutpoint,
};
use kaspa_hashes::Hash;
use kaspa_rpc_core::{RpcTransaction, api::rpc::RpcApi};
use secp256k1::Keypair;
use vprog_tictactoe_driver::{config::Config, scenario};
use vprog_tictactoe_guest::runtime::genesis::GENESIS_PUBKEY;
use vprog_tictactoe_node::{
    TicTacToeExitIndexer,
    da_store::{ExitRecord, SpentMark, exit_roots, get_exit_record, leaf_spent},
    indexer::{
        GameStatus, PlayerEvent, TicTacToeIndexer, scan_games_by_status, scan_player_events,
    },
};
use vprogs_core_types::ResourceId;
use vprogs_node_test_utils::L1Node;
use vprogs_runner::{Elfs, Indexer, RunnerConfig, StartMode, start_runner};
use vprogs_storage_types::Store;
use vprogs_zk_abi::withdrawal::StandardSpk;
use vprogs_zk_backend_risc0_api::{PermissionTreeAccumulator, delegate_entry_spk_hash};
use vprogs_zk_backend_risc0_app_kit::{
    PermissionSpendArgs, PermissionTreeView, build_permission_spend, dev_genesis_keypair,
};
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

    // 5. Start runner in settlement mode (dev stub proofs) with fresh covenant deployment.
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
        prove: true,
        start_mode: Some(StartMode::Fresh),
    };

    let handles = start_runner(
        &runner_cfg,
        &arc_client,
        &params,
        elfs,
        delegate_entry_spk_hash,
        Some(Indexer(Arc::new(TicTacToeIndexer))),
        Some(Arc::new(TicTacToeExitIndexer)),
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

        let a_created = scan_player_events(
            &*store,
            &snapshot,
            &report.player_a_user_id,
            PlayerEvent::Created,
            None,
            100,
        );
        let a_won = scan_player_events(
            &*store,
            &snapshot,
            &report.player_a_user_id,
            PlayerEvent::Won,
            None,
            100,
        );
        let b_joined = scan_player_events(
            &*store,
            &snapshot,
            &report.player_b_user_id,
            PlayerEvent::Joined,
            None,
            100,
        );
        let b_lost = scan_player_events(
            &*store,
            &snapshot,
            &report.player_b_user_id,
            PlayerEvent::Lost,
            None,
            100,
        );
        let finished_games =
            scan_games_by_status(&*store, &snapshot, GameStatus::Finished, None, 100);
        let open_games = scan_games_by_status(&*store, &snapshot, GameStatus::Open, None, 100);

        let has_a_created = a_created.iter().any(|e| e.game == game_bytes);
        let has_a_won = a_won.iter().any(|e| e.game == game_bytes);
        let has_b_joined = b_joined.iter().any(|e| e.game == game_bytes);
        let has_b_lost = b_lost.iter().any(|e| e.game == game_bytes);
        let has_finished = finished_games.contains(&game_bytes);
        let not_open = !open_games.contains(&game_bytes);

        if has_a_created && has_a_won && has_b_joined && has_b_lost && has_finished && not_open {
            indexed = true;
            break;
        }

        tokio::time::sleep(poll_interval).await;
    }

    assert!(indexed, "timed out waiting for secondary index entries after completed match");

    let snapshot = store.canonical_chain().snapshot();

    // 9b. Exact event streams: exactly one canonical entry per (player, event), each naming
    // this game. The scenario creates one game by A (wins) joined by B (loses), so a fresh
    // covenant holds exactly these four entries and nothing else.
    let a_created = scan_player_events(
        &*store,
        &snapshot,
        &report.player_a_user_id,
        PlayerEvent::Created,
        None,
        100,
    );
    let a_won = scan_player_events(
        &*store,
        &snapshot,
        &report.player_a_user_id,
        PlayerEvent::Won,
        None,
        100,
    );
    let b_joined = scan_player_events(
        &*store,
        &snapshot,
        &report.player_b_user_id,
        PlayerEvent::Joined,
        None,
        100,
    );
    let b_lost = scan_player_events(
        &*store,
        &snapshot,
        &report.player_b_user_id,
        PlayerEvent::Lost,
        None,
        100,
    );

    for (label, stream) in
        [("A Created", &a_created), ("A Won", &a_won), ("B Joined", &b_joined), ("B Lost", &b_lost)]
    {
        assert_eq!(stream.len(), 1, "player {label} stream must hold exactly one entry");
        assert_eq!(stream[0].game, game_bytes, "player {label} entry must name the game");
        assert!(
            snapshot.is_canonical(stream[0].version),
            "player {label} version must be canonical"
        );
    }

    // 9c. Status buckets: the lone game ended terminal, so it lives in Finished and only there.
    let finished_games = scan_games_by_status(&*store, &snapshot, GameStatus::Finished, None, 100);
    let open_games = scan_games_by_status(&*store, &snapshot, GameStatus::Open, None, 100);
    let playing_games = scan_games_by_status(&*store, &snapshot, GameStatus::Playing, None, 100);
    assert_eq!(finished_games, vec![game_bytes], "Finished bucket must hold exactly the game");
    assert!(open_games.is_empty(), "Open bucket must be empty once the lone game finished");
    assert!(playing_games.is_empty(), "Playing bucket must be empty once the match settled");

    // 9d. Pagination invariants: walking limit-1 pages from an exclusive cursor concatenates to
    // exactly the unlimited scan, and the page past the last entry is empty. Streams here hold
    // one entry each (multi-entry paging is pinned by node indexer unit tests and needs the
    // open-game driver helper); the empty-bucket cases (Open, Playing) exercise the empty-walk.
    for (player, event) in [
        (&report.player_a_user_id, PlayerEvent::Created),
        (&report.player_a_user_id, PlayerEvent::Won),
        (&report.player_b_user_id, PlayerEvent::Joined),
        (&report.player_b_user_id, PlayerEvent::Lost),
    ] {
        let full = scan_player_events(&*store, &snapshot, player, event, None, 100);
        let mut paged = Vec::new();
        let mut after = None;
        while let Some(&entry) =
            scan_player_events(&*store, &snapshot, player, event, after, 1).first()
        {
            after = Some(entry);
            paged.push(entry);
        }
        assert_eq!(paged, full, "limit-1 paging must reproduce the full {event:?} stream");
        assert!(
            scan_player_events(&*store, &snapshot, player, event, after, 1).is_empty(),
            "page past the last {event:?} entry must be empty"
        );
    }

    for status in [GameStatus::Open, GameStatus::Playing, GameStatus::Finished] {
        let full = scan_games_by_status(&*store, &snapshot, status, None, 100);
        let mut paged = Vec::new();
        let mut after_game = None;
        while let Some(&game) =
            scan_games_by_status(&*store, &snapshot, status, after_game, 1).first()
        {
            after_game = Some(ResourceId::from(game));
            paged.push(game);
        }
        assert_eq!(paged, full, "limit-1 paging must reproduce the full {status:?} bucket");
        assert!(
            scan_games_by_status(&*store, &snapshot, status, after_game, 1).is_empty(),
            "page past the last {status:?} entry must be empty"
        );
    }

    // 10. Exit tail: wait for the settlement to record the withdraw leaf, claim it in full,
    // and verify the payout plus the spend mark. The wait reads the store, not
    // `handles.exits_rx`: the exit indexer owns that channel and closes the returned receiver.
    let dest_spk = StandardSpk::PubKey(&report.player_a_pubkey);
    let (exit_root, record, leaf_index) =
        wait_exit_with_leaf(&*store, dest_spk.to_script_bytes().as_slice()).await;

    // Feed and handler coverage: the record names a real settlement, counts its leaves
    // unclaimed, and carries exactly the winner-pot withdraw.
    let leaf = &record.leaves[leaf_index];
    assert_eq!(leaf.amount, driver_cfg.stake * 2, "exit leaf must carry the winner pot");
    assert_eq!(
        record.unclaimed as usize,
        record.leaves.len(),
        "a freshly recorded bundle counts every leaf unclaimed"
    );
    assert_ne!(record.settlement_txid, [0u8; 32], "record must name the settlement tx");

    // The stored leaves must rebuild the root the record is keyed by.
    let tree = PermissionTreeView::from_leaves(&record.leaves);
    assert_eq!(tree.root(), exit_root, "stored leaves must rebuild the recorded root");

    // Claims pay the leaf out of the delegate pool at the covenant deposit address.
    let covenant_id = handles.covenant_id.as_bytes();
    let deposit_address =
        Address::new(Prefix::Simnet, Version::ScriptHash, &delegate_entry_spk_hash(&covenant_id));
    let delegates = delegate_pool(&arc_client, &deposit_address).await;
    let delegate_total: u64 = delegates.iter().map(|(_, amount)| *amount).sum();
    assert!(
        delegate_total >= leaf.amount,
        "delegate pool {delegate_total} must cover the leaf {}",
        leaf.amount
    );

    // Full-leaf claim. On the last leaf of the root the builder folds the permission rent
    // into the payout, so the expected value depends on the remaining unclaimed count.
    let expected_payout = leaf.amount + if record.unclaimed == 1 { record.rent } else { 0 };
    let claim_txid = submit_full_claim(
        &arc_client,
        &covenant_id,
        &record,
        leaf_index,
        tree.root_with_leaf(leaf_index, PermissionTreeAccumulator::hash_empty()),
        delegates,
    )
    .await;

    // Payout lands at player A's P2PK destination with the full-leaf value.
    let payout_address = Address::new(Prefix::Simnet, Version::PubKey, &report.player_a_pubkey);
    wait_payout(&arc_client, &payout_address, expected_payout).await;

    // Bridge watcher coverage: the spend mark names our claim tx with the full deduct.
    let mark = wait_leaf_spent(&*store, &exit_root, leaf_index).await;
    assert_eq!(mark.spend_txid, claim_txid.as_bytes(), "spend mark must name the claim tx");
    assert_eq!(mark.deduct, leaf.amount, "spend mark must record the full-leaf deduct");

    // Stop background miner and cleanup.
    mining_stop.notify_one();
    let _ = miner_handle.await;
    l1.mine_blocks(2).await;
}

/// Polls the exit index store until a settled record holds a leaf paying `dest_spk`.
///
/// Returns the record's root, the record, and the matched leaf position. Fails the test on
/// timeout, proving the exit feed, the settlement pairing, and the store handler together.
async fn wait_exit_with_leaf<S: Store>(
    store: &S,
    dest_spk: &[u8],
) -> ([u8; 32], ExitRecord, usize) {
    let timeout = Duration::from_secs(180);
    let start = tokio::time::Instant::now();
    loop {
        for root in exit_roots(store) {
            if let Some(record) = get_exit_record(store, &root)
                && let Some(index) = record.leaves.iter().position(|l| l.script_bytes() == dest_spk)
            {
                return (root, record, index);
            }
        }
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for a settled exit record holding the withdraw leaf"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Confirmed UTXO set at `address` as claim-ready `(outpoint, amount)` pairs.
async fn delegate_pool<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    address: &Address,
) -> Vec<(TransactionOutpoint, u64)> {
    client
        .get_utxos_by_addresses(vec![address.clone()])
        .await
        .expect("fetch delegate utxos")
        .into_iter()
        .map(|e| (TransactionOutpoint::from(e.outpoint), e.utxo_entry.amount))
        .collect()
}

/// Builds and submits the full-leaf permission claim, returning the claim txid.
///
/// The post-claim root is caller-supplied (the full-leaf fold empties the leaf slot), and the
/// delegate inputs fund the payout.
async fn submit_full_claim<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    covenant_id: &[u8; 32],
    record: &ExitRecord,
    leaf_index: usize,
    new_root: [u8; 32],
    delegates: Vec<(TransactionOutpoint, u64)>,
) -> Hash {
    let leaf = &record.leaves[leaf_index];
    let tree = PermissionTreeView::from_leaves(&record.leaves);
    let args = PermissionSpendArgs {
        covenant_id: *covenant_id,
        permission_outpoint: TransactionOutpoint::new(
            Hash::from_bytes(record.settlement_txid),
            record.outpoint_index,
        ),
        permission_rent: record.rent,
        old_root: tree.root(),
        old_unclaimed: record.unclaimed,
        depth: tree.depth(),
        leaf_index,
        leaf_spk: leaf.script_bytes(),
        leaf_amount: leaf.amount,
        deduct: leaf.amount,
        siblings: tree.siblings(leaf_index),
        new_root,
        new_unclaimed: record.unclaimed - 1,
        delegate_inputs: delegates,
    };
    let (tx, _utxos) = build_permission_spend(&args).expect("claim assembly failed");
    client
        .submit_transaction(RpcTransaction::from(&tx), false)
        .await
        .expect("claim submission rejected");
    tx.id()
}

/// Polls until a UTXO of exactly `expected` sompi appears at `address`.
async fn wait_payout<C: RpcApi + ?Sized>(client: &Arc<C>, address: &Address, expected: u64) {
    let timeout = Duration::from_secs(60);
    let start = tokio::time::Instant::now();
    loop {
        let entries =
            client.get_utxos_by_addresses(vec![address.clone()]).await.expect("fetch payout utxos");
        if entries.iter().any(|e| e.utxo_entry.amount == expected) {
            return;
        }
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for a {expected}-sompi payout at {address}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Polls the exit index store until the leaf's spend mark lands, returning it.
async fn wait_leaf_spent<S: Store>(store: &S, root: &[u8; 32], leaf_index: usize) -> SpentMark {
    let timeout = Duration::from_secs(60);
    let start = tokio::time::Instant::now();
    loop {
        if let Some(mark) = leaf_spent(store, root, leaf_index) {
            return mark;
        }
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for the spend mark on the claimed leaf"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
