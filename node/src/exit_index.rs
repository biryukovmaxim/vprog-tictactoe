//! Exit indexer for the tic-tac-toe application.
//!
//! Records settled exit bundles and L1 permission spends into the node's index store for DA
//! queries. Each exit family keeps one live [`ExitRecord`] that advances to the claim's
//! continuation on every permission spend, so remaining leaves stay claimable; reorg reverts
//! invert the advance, and orphaned settlements hide until their re-confirmation re-anchors
//! them.

use std::{collections::HashMap, sync::RwLock};

use kaspa_consensus_core::tx::TransactionOutpoint;
use vprogs_l1_types::{PermissionSpend, SettlementInfo};
use vprogs_runner::{ExitIndexer, ExitsForBundle};
use vprogs_storage_types::WriteBatch;
use vprogs_zk_backend_risc0_api::PermissionTreeView;

use crate::da_store::{
    EmptiedLeaf, ExitRecord, LatestSettlement, SpentMark, delete_exit_record, mark_leaf_spent,
    put_exit_record, put_latest_settlement, unmark_leaf_spent,
};

/// Secondary exit indexer recording exit bundles and advancing them on permission spends.
///
/// Settlements and spends arrive in L1 order, so a cold start (or a catchup replay that
/// re-pairs the settlement) always seeds the in-memory mirror below before a family's spends
/// arrive. A warm restart mid-family, whose mirror cannot re-seed without the pairing, hides
/// the affected family from serving instead of serving stale claimable args.
#[derive(Default)]
pub struct TicTacToeExitIndexer {
    /// Live family records keyed by their current raw root; a write-through mirror of the
    /// stored records (the trait hook receives no store handle to read them back).
    records: RwLock<HashMap<[u8; 32], ExitRecord>>,
}

/// Builds a fresh family record for `bundle` under `settlement`'s anchor, keyed by its raw
/// padded root.
// ponytail: rent pinned to DEFAULT_PERMISSION_OUTPUT_VALUE until a per-covenant override exists
fn record_for(bundle: &ExitsForBundle, settlement: &SettlementInfo) -> ([u8; 32], ExitRecord) {
    let rec = ExitRecord {
        settlement_txid: settlement.tx_id.as_bytes(),
        outpoint_index: 1,
        daa_score: settlement.daa_score.get(),
        unclaimed: bundle.leaves.len() as u64,
        rent: 50_000_000,
        leaves: (*bundle.leaves).clone(),
        emptied: Vec::new(),
    };
    // Key on the raw padded root (the redeem's old_root space), not the script-hash
    // commitment the runner publishes.
    (PermissionTreeView::from_leaves(&rec.leaves).root(), rec)
}

impl ExitIndexer for TicTacToeExitIndexer {
    fn on_exits_committed(
        &self,
        bundle: &ExitsForBundle,
        settlement: &SettlementInfo,
        wb: &mut dyn WriteBatch,
    ) {
        let (root, rec) = record_for(bundle, settlement);
        put_exit_record(wb, &root, &rec);
        self.records.write().expect("poisoned lock").insert(root, rec);
        put_latest_settlement(
            wb,
            &LatestSettlement {
                state_root: bundle.new_state,
                permission_root: bundle.permission_spk_hash,
                txid: settlement.tx_id.as_bytes(),
                daa_score: settlement.daa_score.get(),
            },
        );
    }

    fn on_permission_spent(&self, spend: &PermissionSpend, wb: &mut dyn WriteBatch) {
        let mark = SpentMark {
            spend_txid: spend.spend_txid,
            deduct: spend.deduct,
            new_root: spend.new_root,
        };
        mark_leaf_spent(wb, &spend.old_root, spend.leaf_index, &mark);

        let mut records = self.records.write().expect("poisoned lock");
        let Some(mut rec) = records.get(&spend.old_root).cloned() else {
            // Without the mirror the record cannot advance correctly; the spend proves the
            // family's served args are stale, so hide the family rather than serve a claim
            // that would re-spend a spent outpoint.
            log::warn!("permission spend under unseeded root; hiding the exit family");
            delete_exit_record(wb, &spend.old_root);
            return;
        };
        // A drained family, or a replayed spend of an already-emptied leaf, must not advance
        // the record twice.
        if rec.unclaimed == 0
            || rec.emptied.iter().any(|e| e.leaf_index as usize == spend.leaf_index)
        {
            return;
        }
        rec.emptied.push(EmptiedLeaf {
            leaf_index: spend.leaf_index as u32,
            spend_txid: spend.spend_txid,
            deduct: spend.deduct,
        });
        if rec.unclaimed > 1 {
            // Non-terminal: the claim emits a continuation permission output, so the record
            // re-keys onto it.
            delete_exit_record(wb, &spend.old_root);
            records.remove(&spend.old_root);
            rec.settlement_txid = spend.spend_txid;
            rec.outpoint_index = spend.new_outpoint_index;
            rec.unclaimed -= 1;
            records.insert(spend.new_root, rec.clone());
            put_exit_record(wb, &spend.new_root, &rec);
        } else {
            // Terminal: no continuation output exists, so the record drains in place.
            rec.unclaimed = 0;
            records.insert(spend.old_root, rec.clone());
            put_exit_record(wb, &spend.old_root, &rec);
        }
    }

    fn on_permission_spend_reverted(
        &self,
        spend: &PermissionSpend,
        spent_outpoint: &TransactionOutpoint,
        wb: &mut dyn WriteBatch,
    ) {
        // The apply-side mark is unconditional, so the unmark must precede any early
        // return; idempotent, and keyed fully by the spend itself.
        unmark_leaf_spent(wb, &spend.old_root, spend.leaf_index);
        let was_spent = |e: &EmptiedLeaf| {
            e.leaf_index as usize == spend.leaf_index && e.spend_txid == spend.spend_txid
        };
        let mut records = self.records.write().expect("poisoned lock");
        // A non-terminal advance re-keyed the record onto new_root; a terminal one kept old_root.
        // Prefer whichever record actually carries this spend.
        let Some(key) = [&spend.old_root, &spend.new_root]
            .into_iter()
            .find(|&key| records.get(key).is_some_and(|rec| rec.emptied.iter().any(was_spent)))
        else {
            // The family may already be hidden by a pairing revert that landed first; benign.
            return;
        };
        let mut rec = records.remove(key).expect("key found above");
        rec.emptied.retain(|e| !was_spent(e));
        rec.unclaimed += 1;
        if *key == spend.new_root {
            // Non-terminal: the advance overwrote the record's anchor fields; undo both.
            delete_exit_record(wb, key);
            rec.settlement_txid = spent_outpoint.transaction_id.as_bytes();
            rec.outpoint_index = spent_outpoint.index;
        }
        records.insert(spend.old_root, rec.clone());
        put_exit_record(wb, &spend.old_root, &rec);
    }

    fn on_exits_reverted(
        &self,
        bundle: &ExitsForBundle,
        _settlement: &SettlementInfo,
        wb: &mut dyn WriteBatch,
    ) {
        // Anchors stay parked in the runner's registry; only serving hides until re-anchor.
        // A claim advance re-keyed the record onto its continuation root, but leaves are
        // invariant across advances, so locate the live record by leaf equality and fall
        // back to the bundle-derived root when the mirror holds no match.
        // ponytail: identical leaf sets across families hide an arbitrary one; key by
        // txid-root pair if that collision ever bites.
        let key = self
            .records
            .read()
            .expect("poisoned lock")
            .iter()
            .find(|(_, rec)| rec.leaves == *bundle.leaves)
            .map(|(key, _)| *key)
            .unwrap_or_else(|| PermissionTreeView::from_leaves(&bundle.leaves).root());
        delete_exit_record(wb, &key);
        self.records.write().expect("poisoned lock").remove(&key);
    }

    fn on_exits_recommitted(
        &self,
        bundle: &ExitsForBundle,
        settlement: &SettlementInfo,
        wb: &mut dyn WriteBatch,
    ) {
        // Claims cannot exist above a reverted settlement, so the record rebuilds fresh.
        let (root, rec) = record_for(bundle, settlement);
        put_exit_record(wb, &root, &rec);
        self.records.write().expect("poisoned lock").insert(root, rec);
        // The re-anchor must also refresh the served latest-settlement row.
        put_latest_settlement(
            wb,
            &LatestSettlement {
                state_root: bundle.new_state,
                permission_root: bundle.permission_spk_hash,
                txid: settlement.tx_id.as_bytes(),
                daa_score: settlement.daa_score.get(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kaspa_consensus_core::tx::{TransactionId as KaspaTransactionId, TransactionOutpoint};
    use tempfile::TempDir;
    use vprogs_l1_types::{PermissionSpend, SettlementInfo, TransactionId};
    use vprogs_runner::{ExitIndexer, ExitLeaf, ExitsForBundle};
    use vprogs_storage_rocksdb_store::RocksDbStore;
    use vprogs_storage_types::Store;
    use vprogs_zk_abi::withdrawal::StandardSpk;
    use vprogs_zk_backend_risc0_api::{PermissionTreeAccumulator, PermissionTreeView};
    use zerocopy::little_endian::U64;

    use super::*;
    use crate::da_store::{exit_views, get_exit_record, latest_settlement, leaf_spent};

    #[test]
    fn test_exit_indexer_records_and_advances_exit_family() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeExitIndexer::default();

        let pk = [0x12; 32];
        let leaves = vec![
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk), 50_000_000),
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk), 25_000_000),
        ];
        let bundle = ExitsForBundle {
            new_state: [0xaa; 32],
            permission_spk_hash: [0xbb; 32],
            leaves: Arc::new(leaves.clone()),
        };
        let settlement = SettlementInfo {
            tx_id: TransactionId::from_bytes([0xcc; 32]),
            daa_score: U64::new(789_101),
            ..Default::default()
        };

        let mut wb = store.write_batch();
        indexer.on_exits_committed(&bundle, &settlement, &mut wb);
        store.commit(wb);

        // The record keys on the raw padded root, not the script-hash commitment.
        let tree = PermissionTreeView::from_leaves(&leaves);
        let root0 = tree.root();
        let expected_record = ExitRecord {
            settlement_txid: [0xcc; 32],
            outpoint_index: 1,
            daa_score: 789_101,
            unclaimed: 2,
            rent: 50_000_000,
            leaves: leaves.clone(),
            emptied: Vec::new(),
        };
        assert_eq!(get_exit_record(&store, &root0), Some(expected_record));
        assert_eq!(get_exit_record(&store, &bundle.permission_spk_hash), None);

        // The latest-settlement row keeps the runner's script-hash commitment.
        assert_eq!(
            latest_settlement(&store),
            Some(LatestSettlement {
                state_root: [0xaa; 32],
                permission_root: [0xbb; 32],
                txid: [0xcc; 32],
                daa_score: 789_101,
            })
        );

        // First claim: the true continuation root folds the claimed slot to the empty hash.
        let empty = PermissionTreeAccumulator::hash_empty();
        let root1 = tree.root_with_leaf(0, empty);
        let spend1 = PermissionSpend {
            covenant_id: [0x01; 32],
            old_root: root0,
            old_unclaimed: 2,
            depth: tree.depth(),
            leaf_index: 0,
            leaf_spk_bytes: vec![0x02; 32],
            leaf_amount: 50_000_000,
            deduct: 50_000_000,
            new_root: root1,
            spend_txid: [0xee; 32],
            new_outpoint_index: 1,
            chain_idx: 10,
        };

        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend1, &mut wb);
        store.commit(wb);

        // The spend mark keys under the root the redeem spent.
        assert_eq!(
            leaf_spent(&store, &root0, 0),
            Some(SpentMark { spend_txid: [0xee; 32], deduct: 50_000_000, new_root: root1 })
        );
        assert_eq!(get_exit_record(&store, &root0), None, "advanced record must re-key");
        assert_eq!(
            get_exit_record(&store, &root1),
            Some(ExitRecord {
                settlement_txid: [0xee; 32],
                outpoint_index: 1,
                daa_score: 789_101,
                unclaimed: 1,
                rent: 50_000_000,
                leaves: leaves.clone(),
                emptied: vec![EmptiedLeaf {
                    leaf_index: 0,
                    spend_txid: [0xee; 32],
                    deduct: 50_000_000,
                }],
            })
        );

        // Second claim drains the family: the record stays under the same key with unclaimed 0.
        let root2 = PermissionTreeAccumulator::hash_branch(&empty, &empty);
        let spend2 = PermissionSpend {
            covenant_id: [0x01; 32],
            old_root: root1,
            old_unclaimed: 1,
            depth: tree.depth(),
            leaf_index: 1,
            leaf_spk_bytes: vec![0x02; 32],
            leaf_amount: 25_000_000,
            deduct: 25_000_000,
            new_root: root2,
            spend_txid: [0xef; 32],
            new_outpoint_index: 1,
            chain_idx: 11,
        };

        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend2, &mut wb);
        store.commit(wb);

        let drained = get_exit_record(&store, &root1).expect("terminal record keeps its key");
        assert_eq!(drained.unclaimed, 0);
        assert_eq!(drained.emptied.len(), 2);
        assert_eq!(
            leaf_spent(&store, &root1, 1),
            Some(SpentMark { spend_txid: [0xef; 32], deduct: 25_000_000, new_root: root2 })
        );

        // Replaying the terminal spend must not advance or duplicate anything.
        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend2, &mut wb);
        store.commit(wb);
        assert_eq!(get_exit_record(&store, &root1), Some(drained));
    }

    #[test]
    fn test_unseeded_spend_hides_family_but_writes_mark() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        // A record restored from storage but a cold mirror: a restarted indexer.
        let indexer = TicTacToeExitIndexer::default();

        let leaves = vec![ExitLeaf::from_pair(StandardSpk::PubKey(&[0x12; 32]), 50_000_000)];
        let root = PermissionTreeView::from_leaves(&leaves).root();
        let rec = ExitRecord {
            settlement_txid: [0xcc; 32],
            outpoint_index: 1,
            daa_score: 1,
            unclaimed: 1,
            rent: 50_000_000,
            leaves,
            emptied: Vec::new(),
        };
        let mut wb = store.write_batch();
        put_exit_record(&mut wb, &root, &rec);
        store.commit(wb);

        let spend = PermissionSpend {
            covenant_id: [0x01; 32],
            old_root: root,
            old_unclaimed: 1,
            depth: 1,
            leaf_index: 0,
            leaf_spk_bytes: vec![0x02; 32],
            leaf_amount: 50_000_000,
            deduct: 50_000_000,
            new_root: [0xdd; 32],
            spend_txid: [0xee; 32],
            new_outpoint_index: 1,
            chain_idx: 12,
        };
        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend, &mut wb);
        store.commit(wb);

        assert_eq!(get_exit_record(&store, &root), None, "unservable family must be hidden");
        assert_eq!(
            leaf_spent(&store, &root, 0),
            Some(SpentMark { spend_txid: [0xee; 32], deduct: 50_000_000, new_root: [0xdd; 32] })
        );
    }

    /// A committed two-leaf family and the two spends that would drain it leaf by leaf.
    struct Family {
        /// Canonical leaves of the settled family.
        leaves: Vec<ExitLeaf>,
        /// The settled bundle as the runner delivered it.
        bundle: ExitsForBundle,
        /// The settlement that paired the bundle.
        settlement: SettlementInfo,
        /// Raw padded root of the freshly settled family.
        root0: [u8; 32],
        /// Continuation root after the first claim.
        root1: [u8; 32],
        /// First claim: leaf 0, non-terminal.
        spend1: PermissionSpend,
        /// Second claim: leaf 1, terminal.
        spend2: PermissionSpend,
    }

    /// Commits a two-leaf family into `store` via `indexer`, leaving both leaves unclaimed.
    fn commit_two_leaf_family(store: &RocksDbStore, indexer: &TicTacToeExitIndexer) -> Family {
        let pk = [0x12; 32];
        let leaves = vec![
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk), 50_000_000),
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk), 25_000_000),
        ];
        let bundle = ExitsForBundle {
            new_state: [0xaa; 32],
            permission_spk_hash: [0xbb; 32],
            leaves: Arc::new(leaves.clone()),
        };
        let settlement = SettlementInfo {
            tx_id: TransactionId::from_bytes([0xcc; 32]),
            daa_score: U64::new(789_101),
            ..Default::default()
        };
        let mut wb = store.write_batch();
        indexer.on_exits_committed(&bundle, &settlement, &mut wb);
        store.commit(wb);

        let tree = PermissionTreeView::from_leaves(&leaves);
        let empty = PermissionTreeAccumulator::hash_empty();
        let root0 = tree.root();
        let root1 = tree.root_with_leaf(0, empty);
        let root2 = PermissionTreeAccumulator::hash_branch(&empty, &empty);
        Family {
            leaves,
            spend1: PermissionSpend {
                covenant_id: [0x01; 32],
                old_root: root0,
                old_unclaimed: 2,
                depth: tree.depth(),
                leaf_index: 0,
                leaf_spk_bytes: vec![0x02; 32],
                leaf_amount: 50_000_000,
                deduct: 50_000_000,
                new_root: root1,
                spend_txid: [0xee; 32],
                new_outpoint_index: 1,
                chain_idx: 10,
            },
            spend2: PermissionSpend {
                covenant_id: [0x01; 32],
                old_root: root1,
                old_unclaimed: 1,
                depth: tree.depth(),
                leaf_index: 1,
                leaf_spk_bytes: vec![0x02; 32],
                leaf_amount: 25_000_000,
                deduct: 25_000_000,
                new_root: root2,
                spend_txid: [0xef; 32],
                new_outpoint_index: 1,
                chain_idx: 11,
            },
            bundle,
            settlement,
            root0,
            root1,
        }
    }

    #[test]
    fn revert_non_terminal_spend_rekeys_back() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeExitIndexer::default();
        let fam = commit_two_leaf_family(&store, &indexer);

        let mut wb = store.write_batch();
        indexer.on_permission_spent(&fam.spend1, &mut wb);
        store.commit(wb);

        // The revert's spent_outpoint is the pre-spend anchor: the settlement's permission output.
        let spent = TransactionOutpoint::new(KaspaTransactionId::from_bytes([0xcc; 32]), 1);
        let mut wb = store.write_batch();
        indexer.on_permission_spend_reverted(&fam.spend1, &spent, &mut wb);
        store.commit(wb);

        let rec = get_exit_record(&store, &fam.root0).expect("record must re-key back to root0");
        assert_eq!(rec.unclaimed, 2);
        assert_eq!(rec.emptied, Vec::new());
        assert_eq!(rec.settlement_txid, [0xcc; 32]);
        assert_eq!(rec.outpoint_index, 1);
        assert_eq!(leaf_spent(&store, &fam.root0, 0), None);
        assert_eq!(get_exit_record(&store, &fam.root1), None);
    }

    #[test]
    fn revert_terminal_spend_undrains() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeExitIndexer::default();
        let fam = commit_two_leaf_family(&store, &indexer);

        let mut wb = store.write_batch();
        indexer.on_permission_spent(&fam.spend1, &mut wb);
        store.commit(wb);
        let mut wb = store.write_batch();
        indexer.on_permission_spent(&fam.spend2, &mut wb);
        store.commit(wb);

        // The terminal spend consumed the first claim's continuation output.
        let spent = TransactionOutpoint::new(KaspaTransactionId::from_bytes([0xee; 32]), 1);
        let mut wb = store.write_batch();
        indexer.on_permission_spend_reverted(&fam.spend2, &spent, &mut wb);
        store.commit(wb);

        let rec = get_exit_record(&store, &fam.root1).expect("terminal record keeps its key");
        assert_eq!(rec.unclaimed, 1);
        assert_eq!(
            rec.emptied,
            vec![EmptiedLeaf { leaf_index: 0, spend_txid: [0xee; 32], deduct: 50_000_000 }]
        );
        assert_eq!(leaf_spent(&store, &fam.root1, 1), None);
    }

    #[test]
    fn revert_of_unknown_spend_is_noop() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeExitIndexer::default();

        // No family was ever committed; the revert must neither panic nor write.
        let spend = PermissionSpend {
            covenant_id: [0x01; 32],
            old_root: [0xa1; 32],
            old_unclaimed: 1,
            depth: 1,
            leaf_index: 0,
            leaf_spk_bytes: vec![0x02; 32],
            leaf_amount: 50_000_000,
            deduct: 50_000_000,
            new_root: [0xa2; 32],
            spend_txid: [0xee; 32],
            new_outpoint_index: 1,
            chain_idx: 10,
        };
        // The apply-side mark is unconditional, so seed one even though the family is
        // unknown to the mirror; the revert must clear it despite no-oping elsewhere.
        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend, &mut wb);
        store.commit(wb);
        assert!(leaf_spent(&store, &[0xa1; 32], 0).is_some());

        let spent = TransactionOutpoint::new(KaspaTransactionId::from_bytes([0xcc; 32]), 1);
        let mut wb = store.write_batch();
        indexer.on_permission_spend_reverted(&spend, &spent, &mut wb);
        store.commit(wb);

        assert_eq!(get_exit_record(&store, &[0xa1; 32]), None);
        assert_eq!(get_exit_record(&store, &[0xa2; 32]), None);
        assert_eq!(leaf_spent(&store, &[0xa1; 32], 0), None);
        assert!(exit_views(&store).is_empty());
    }

    #[test]
    fn exits_reverted_hides_claim_advanced_family() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeExitIndexer::default();
        let fam = commit_two_leaf_family(&store, &indexer);

        // A non-terminal claim re-keys the record onto its continuation root.
        let mut wb = store.write_batch();
        indexer.on_permission_spent(&fam.spend1, &mut wb);
        store.commit(wb);
        assert!(get_exit_record(&store, &fam.root1).is_some());

        // The settlement reverts under the original bundle; the advanced record must
        // hide even though its key moved off the bundle-derived root.
        let mut wb = store.write_batch();
        indexer.on_exits_reverted(&fam.bundle, &fam.settlement, &mut wb);
        store.commit(wb);

        assert_eq!(get_exit_record(&store, &fam.root0), None);
        assert_eq!(get_exit_record(&store, &fam.root1), None);
        assert!(exit_views(&store).is_empty());
    }

    #[test]
    fn exits_reverted_hides_and_recommit_reanchors() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeExitIndexer::default();
        let fam = commit_two_leaf_family(&store, &indexer);

        let mut wb = store.write_batch();
        indexer.on_exits_reverted(&fam.bundle, &fam.settlement, &mut wb);
        store.commit(wb);
        assert_eq!(get_exit_record(&store, &fam.root0), None, "reverted family must hide");
        assert!(exit_views(&store).is_empty());

        // The settlement re-confirms under a fresh anchor: same family, new txid and DAA score.
        let fresh = SettlementInfo {
            tx_id: TransactionId::from_bytes([0xcd; 32]),
            daa_score: U64::new(999_222),
            ..Default::default()
        };
        let mut wb = store.write_batch();
        indexer.on_exits_recommitted(&fam.bundle, &fresh, &mut wb);
        store.commit(wb);

        let rec = get_exit_record(&store, &fam.root0).expect("recommit restores the record");
        assert_eq!(rec.settlement_txid, [0xcd; 32]);
        assert_eq!(rec.daa_score, 999_222);
        assert_eq!(rec.unclaimed, fam.leaves.len() as u64);
        assert_eq!(rec.leaves, fam.leaves);
        assert_eq!(rec.emptied, Vec::new());

        // The re-anchor refreshes the served latest-settlement row, not just the record.
        assert_eq!(
            latest_settlement(&store),
            Some(LatestSettlement {
                state_root: fam.bundle.new_state,
                permission_root: fam.bundle.permission_spk_hash,
                txid: [0xcd; 32],
                daa_score: 999_222,
            })
        );
    }
}
