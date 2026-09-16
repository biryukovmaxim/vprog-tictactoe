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
    config::params::{ForkActivation, Params},
    hashing::sighash_type::SIG_HASH_ALL,
    mass::BlockMassLimits,
    network::{NetworkId, NetworkType},
    sign::sign_input,
    tx::{PopulatedTransaction, ScriptPublicKey, Transaction, TransactionOutpoint},
};
use kaspa_hashes::Hash;
use kaspa_rpc_core::{RpcTransaction, api::rpc::RpcApi};
use secp256k1::Keypair;
use vprog_tictactoe_driver::{config::Config, scenario};
use vprog_tictactoe_guest::runtime::genesis::GENESIS_PUBKEY;
use vprog_tictactoe_node::{
    TicTacToeExitIndexer,
    da_store::{
        EmptiedLeaf, ExitRecord, ExitView, SpentMark, exit_roots, exit_views, get_exit_record,
        leaf_spent,
    },
    indexer::{
        GameStatus, PlayerEvent, TicTacToeIndexer, scan_games_by_status, scan_player_events,
    },
};
use vprogs_core_types::ResourceId;
use vprogs_l1_wallet::build::commit_storage_mass;
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
    // Capture the L1 sink before the deploy: the exec-follower below joins the covenant with
    // `start_from` pinned to a block at/before the bootstrap.
    let deploy_anchor = arc_client.get_block_dag_info().await.expect("read L1 sink").sink;
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
        Some(Arc::new(TicTacToeExitIndexer::default())),
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
    // unclaimed, and carries the winner-pot withdraw plus the deposit remainder (one carrier,
    // two leaves), so the claims below chain on a single root.
    let leaf = &record.leaves[leaf_index];
    assert_eq!(record.leaves.len(), 2, "the withdraw carrier settles two exit leaves");
    assert_eq!(leaf.amount, driver_cfg.stake * 2, "exit leaf must carry the winner pot");
    assert_eq!(
        record.unclaimed as usize,
        record.leaves.len(),
        "a freshly recorded bundle counts every leaf unclaimed"
    );
    assert_ne!(record.settlement_txid, [0u8; 32], "record must name the settlement tx");

    // The record key is raw-root space (the redeem's old_root): the stored leaves rebuild it
    // through the padded view, and it differs from the accumulator's script-hash commitment.
    let mut acc = PermissionTreeAccumulator::new();
    for leaf in &record.leaves {
        acc.add_exit(leaf.to_standard_spk(), leaf.amount);
    }
    let tree = PermissionTreeView::from_leaves(&record.leaves);
    assert_eq!(tree.root(), acc.root(), "view root must equal the accumulator's padded root");
    assert_eq!(tree.root(), exit_root, "record must key on the raw padded root");
    assert_ne!(acc.finalize(), exit_root, "raw root must differ from the script-hash commitment");

    // 10b. Exec-follower phase: a second, keyless runner in exec mode joins the same covenant
    // and rebuilds the same exit index purely from L1 observation. Depends only on the settled
    // record, so it runs before the delegate-pool/claim tail.
    //
    // Red-first: like the claim tail below, this phase is expected to fail until the vprogs
    // bridge seq-commit defect is fixed: the whole exit section times out at its first store
    // wait today.
    let follower_dir = tempfile::tempdir().expect("follower tempdir");
    let follower_cfg = RunnerConfig {
        wrpc_url: wrpc_url.clone(),
        private_key: None,
        network_id,
        program_elf: None,
        batch_elf: None,
        aggregator_elf: None,
        data_dir: follower_dir.path().to_path_buf(),
        lane_id: Some(1),
        covenant_id: Some(handles.covenant_id),
        bootstrap_txid: None,
        start_from: Some(deploy_anchor),
        seed_depth: 500,
        min_confirmations: None,
        prove: false,
        start_mode: Some(StartMode::Catchup),
    };
    let follower = start_runner(
        &follower_cfg,
        &arc_client,
        &params,
        elfs,
        delegate_entry_spk_hash,
        Some(Indexer(Arc::new(TicTacToeIndexer))),
        Some(Arc::new(TicTacToeExitIndexer::default())),
    )
    .await
    .expect("follower start_runner failed");
    let follower_store = follower.node.api().storage().store().clone();
    let (_, follower_record, follower_leaf_index) =
        wait_exit_with_leaf(&*follower_store, dest_spk.to_script_bytes().as_slice()).await;
    assert_eq!(follower_record, record, "follower must hold the same exit record");
    assert_eq!(follower_leaf_index, leaf_index, "follower must resolve the same leaf");
    drop(follower);

    // Claims pay the leaf out of the delegate pool at the covenant deposit address and burn a
    // feerate-priced fee from the claimer's own collateral UTXO (delegates are conserved
    // exact), so they enter the mempool through the ordinary submit path and the continuous
    // miner picks them up; no /inject, exactly like a real node.
    let covenant_id = handles.covenant_id.as_bytes();
    let deposit_address =
        Address::new(Prefix::Simnet, Version::ScriptHash, &delegate_entry_spk_hash(&covenant_id));
    let payout_address = Address::new(Prefix::Simnet, Version::PubKey, &report.player_a_pubkey);
    // Fund player A's payout address so the claims have their own collateral to spend.
    l1.fund_address(&payout_address, 1_000_000_000, 2).await;
    l1.mine_blocks(2).await;

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
    let collateral = own_collateral(&arc_client, &payout_address, &report.player_a_secret).await;
    let (claim_tx, claim_txid) = build_full_claim(
        &arc_client,
        &params,
        &covenant_id,
        &record,
        leaf_index,
        tree.root_with_leaf(leaf_index, PermissionTreeAccumulator::hash_empty()),
        delegates,
        &collateral,
    )
    .await;
    let submitted = arc_client
        .submit_transaction(RpcTransaction::from(&claim_tx), false)
        .await
        .expect("fee-bearing claim must enter the mempool");
    assert_eq!(submitted, claim_txid, "the node must accept the claim under its own txid");

    // Payout lands at player A's P2PK destination with the full-leaf value.
    wait_payout(&arc_client, &payout_address, expected_payout).await;

    // Bridge watcher coverage: the spend mark names our claim tx with the full deduct. Marks
    // key on the raw root (the redeem's old_root), which is also the record key.
    let mark = wait_leaf_spent(&*store, &tree.root(), leaf_index).await;
    assert_eq!(mark.spend_txid, claim_txid.as_bytes(), "spend mark must name the claim tx");
    assert_eq!(mark.deduct, leaf.amount, "spend mark must record the full-leaf deduct");

    // 11. Sequential claims: the record must advance onto the first claim's continuation, and
    // the second claim must build purely from the served view (the raw-root serving the web
    // path rides on).
    let other_index = 1 - leaf_index;
    let view = wait_exit_view(&*store, &mark.new_root).await;
    assert_eq!(
        view.record.settlement_txid,
        claim_txid.as_bytes(),
        "advanced record must name the claim tx as its permission source"
    );
    assert_eq!(view.record.outpoint_index, 1, "continuation permission output is index 1");
    assert_eq!(view.record.unclaimed, 1, "one leaf claimed, one remains");
    assert_eq!(
        view.spent[leaf_index],
        Some(EmptiedLeaf {
            leaf_index: leaf_index as u32,
            spend_txid: claim_txid.as_bytes(),
            deduct: leaf.amount,
        }),
        "the claimed leaf serves its emptied entry"
    );
    assert!(view.spent[other_index].is_none(), "the remaining leaf stays unclaimed");

    let leaf2 = &view.record.leaves[other_index];
    let delegates2 = delegate_pool(&arc_client, &deposit_address).await;
    let delegate2_total: u64 = delegates2.iter().map(|(_, amount)| *amount).sum();
    assert!(
        delegate2_total >= leaf2.amount,
        "delegate pool {delegate2_total} must cover the second leaf {}",
        leaf2.amount
    );
    let collateral2 = own_collateral(&arc_client, &payout_address, &report.player_a_secret).await;

    let (claim2_tx, claim2_txid) = build_claim_from_view(
        &arc_client,
        &covenant_id,
        &view,
        other_index,
        delegates2,
        &collateral2,
    )
    .await;
    let submitted2 = arc_client
        .submit_transaction(RpcTransaction::from(&claim2_tx), false)
        .await
        .expect("second fee-bearing claim must enter the mempool");
    assert_eq!(submitted2, claim2_txid);

    // Terminal payout: the last leaf folds the permission rent into the payout. Match by the
    // claim's own outpoint; the first payout shares the destination and value family.
    let expected_payout2 = leaf2.amount + view.record.rent;
    wait_payout_from(&arc_client, &payout_address, claim2_txid, expected_payout2).await;

    // The family drains in place: same key, unclaimed 0, both leaves spent.
    let mark2 = wait_leaf_spent(&*store, &mark.new_root, other_index).await;
    assert_eq!(mark2.spend_txid, claim2_txid.as_bytes(), "second spend mark names claim 2");
    assert_eq!(mark2.deduct, leaf2.amount, "second spend mark records the full-leaf deduct");
    let drained = wait_exit_view(&*store, &mark.new_root).await;
    assert_eq!(drained.record.unclaimed, 0, "the drained family counts nothing unclaimed");
    assert!(drained.spent.iter().all(|s| s.is_some()), "both leaves serve their emptied entries");

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

/// The claimer's fee collateral: player A's largest own UTXO plus the signing key.
struct Collateral {
    /// The funding outpoint.
    outpoint: TransactionOutpoint,
    /// The UTXO's value in sompi.
    amount: u64,
    /// The UTXO's script public key (a schnorr P2PK paying A's key).
    spk: ScriptPublicKey,
    /// Player A's private key, signing the collateral input.
    secret: [u8; 32],
}

/// Picks the claimer's largest UTXO at `address` as the fee collateral.
async fn own_collateral<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    address: &Address,
    secret: &[u8; 32],
) -> Collateral {
    let entries =
        client.get_utxos_by_addresses(vec![address.clone()]).await.expect("fetch collateral utxos");
    let pick = entries
        .iter()
        .max_by_key(|e| e.utxo_entry.amount)
        .expect("the claimer must own a collateral UTXO");
    Collateral {
        outpoint: TransactionOutpoint::from(pick.outpoint),
        amount: pick.utxo_entry.amount,
        spk: pick.utxo_entry.script_public_key.clone(),
        secret: *secret,
    }
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

/// Builds the full-leaf permission claim, returning it and its txid.
///
/// The post-claim root is caller-supplied (the full-leaf fold empties the leaf slot), the
/// delegate inputs fund the payout, and the collateral input funds a feerate-priced fee burned
/// from its trailing change (signed with A's key, so the claim rides the mempool's ordinary
/// submit path; no /inject).
#[allow(clippy::too_many_arguments)]
async fn build_full_claim<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    params: &Params,
    covenant_id: &[u8; 32],
    record: &ExitRecord,
    leaf_index: usize,
    new_root: [u8; 32],
    delegates: Vec<(TransactionOutpoint, u64)>,
    collateral: &Collateral,
) -> (Transaction, Hash) {
    let leaf = &record.leaves[leaf_index];
    let tree = PermissionTreeView::from_leaves(&record.leaves);
    let build = |fee: u64| {
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
            delegate_inputs: delegates.clone(),
            collateral_input: (collateral.outpoint, collateral.amount),
            collateral_spk: collateral.spk.clone(),
            fee,
            collateral_sig: Vec::new(),
        };
        build_permission_spend(&args)
    };

    // The fee never changes the tx byte length: probe at 0, price from the node's feerate
    // estimation, then rebuild with the real fee.
    let probe = build(0).expect("probe claim assembly failed").0;
    let fee = claim_fee(client, &probe).await;
    let (mut tx, utxos) = build(fee).expect("claim assembly failed");

    // Sign the collateral input over the built transaction (the sighash excludes signature
    // scripts), then commit the KIP-0009 storage mass over the final outputs: Toccata txs
    // must carry it or the node disqualifies their block.
    let idx = tx.inputs.len() - 1;
    let sig = sign_input(
        &PopulatedTransaction::new(&tx, utxos.clone()),
        idx,
        &collateral.secret,
        SIG_HASH_ALL,
    );
    tx.inputs[idx].signature_script = sig;
    tx.finalize();
    commit_storage_mass(params, &tx, &utxos);
    let txid = tx.id();
    (tx, txid)
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

/// Polls the exit store until the advancing family serves its view under `root`.
async fn wait_exit_view<S: Store>(store: &S, root: &[u8; 32]) -> ExitView {
    let timeout = Duration::from_secs(60);
    let start = tokio::time::Instant::now();
    loop {
        if let Some(view) = exit_views(store).into_iter().find(|v| v.root == *root) {
            return view;
        }
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for the exit view under the advanced root"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Builds a full-leaf claim from a served exit view through the web encoder's `claim_tx`
/// builder, so the L1 consensus exercises the exact transaction the frontend submits. Every
/// argument comes from the served view, mirroring the web `claimArgs` mapping; the fee prices
/// from the node's feerate estimation (probe at 0, rebuild with the real fee) and burns from
/// the claimer's collateral UTXO.
async fn build_claim_from_view<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    covenant_id: &[u8; 32],
    view: &ExitView,
    leaf_index: usize,
    delegates: Vec<(TransactionOutpoint, u64)>,
    collateral: &Collateral,
) -> (Transaction, Hash) {
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let leaf = &view.record.leaves[leaf_index];
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
    let build = |fee: u64| {
        vprog_tictactoe_encoder_wasm::claim_tx(
            &privkey_hex,
            &Hash::from_bytes(*covenant_id).to_string(),
            &Hash::from_bytes(view.record.settlement_txid).to_string(),
            view.record.outpoint_index,
            view.record.rent,
            &Hash::from_bytes(view.root).to_string(),
            view.record.unclaimed,
            view.depth as u32,
            leaf_index as u32,
            &hex(leaf.script_bytes()),
            leaf.amount,
            &Hash::from_bytes(view.full_claim_roots[leaf_index]).to_string(),
            view.record.unclaimed - 1,
            view.siblings[leaf_index].iter().map(|s| hex(s.as_slice())).collect(),
            delegate_utxos.clone(),
            collateral_utxo.clone(),
            fee,
        )
    };

    let probe = build(0).expect("encoder probe claim assembly failed");
    let probe_tx: Transaction = borsh::from_slice(&probe).expect("decode encoder probe claim");
    let fee = claim_fee(client, &probe_tx).await;
    let bytes = build(fee).expect("encoder claim assembly failed");
    let tx: Transaction = borsh::from_slice(&bytes).expect("decode encoder claim");
    let txid = tx.id();
    (tx, txid)
}

/// Polls until the claim tx's own output of exactly `expected` sompi appears at `address`.
///
/// Matching by the claim's outpoint (not just amount) keeps the assert meaningful when earlier
/// payouts share the destination and value family.
async fn wait_payout_from<C: RpcApi + ?Sized>(
    client: &Arc<C>,
    address: &Address,
    claim_txid: Hash,
    expected: u64,
) {
    let timeout = Duration::from_secs(60);
    let start = tokio::time::Instant::now();
    loop {
        let entries =
            client.get_utxos_by_addresses(vec![address.clone()]).await.expect("fetch payout utxos");
        if entries.iter().any(|e| {
            TransactionOutpoint::from(e.outpoint).transaction_id == claim_txid
                && e.utxo_entry.amount == expected
        }) {
            return;
        }
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for the {expected}-sompi payout from {claim_txid} at {address}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
