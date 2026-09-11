//! Exit indexer for the tic-tac-toe application.
//!
//! Records settled exit bundles and L1 permission spends into the node's index store for DA
//! queries.

use vprogs_l1_types::{PermissionSpend, SettlementInfo};
use vprogs_runner::{ExitIndexer, ExitsForBundle};
use vprogs_storage_types::WriteBatch;

use crate::da_store::{
    ExitRecord, LatestSettlement, SpentMark, mark_leaf_spent, put_exit_record,
    put_latest_settlement,
};

/// Secondary exit indexer recording exit bundles and leaf spend marks.
pub struct TicTacToeExitIndexer;

impl ExitIndexer for TicTacToeExitIndexer {
    fn on_exits_committed(
        &self,
        bundle: &ExitsForBundle,
        settlement: &SettlementInfo,
        wb: &mut dyn WriteBatch,
    ) {
        // ponytail: rent pinned to DEFAULT_PERMISSION_OUTPUT_VALUE until a per-covenant override
        // exists
        let rec = ExitRecord {
            settlement_txid: settlement.tx_id.as_bytes(),
            outpoint_index: 1,
            daa_score: settlement.daa_score.get(),
            unclaimed: bundle.leaves.len() as u64,
            rent: 50_000_000,
            leaves: (*bundle.leaves).clone(),
        };
        put_exit_record(wb, &bundle.permission_spk_hash, &rec);
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
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;
    use vprogs_l1_types::{PermissionSpend, SettlementInfo, TransactionId};
    use vprogs_runner::{ExitIndexer, ExitLeaf, ExitsForBundle};
    use vprogs_storage_rocksdb_store::RocksDbStore;
    use vprogs_storage_types::Store;
    use vprogs_zk_abi::withdrawal::StandardSpk;
    use zerocopy::little_endian::U64;

    use super::*;
    use crate::da_store::{get_exit_record, latest_settlement, leaf_spent};

    #[test]
    fn test_exit_indexer_records_bundle_and_spend_mark() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeExitIndexer;

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

        let expected_record = ExitRecord {
            settlement_txid: [0xcc; 32],
            outpoint_index: 1,
            daa_score: 789_101,
            unclaimed: 2,
            rent: 50_000_000,
            leaves,
        };
        assert_eq!(get_exit_record(&store, &bundle.permission_spk_hash), Some(expected_record));

        assert_eq!(
            latest_settlement(&store),
            Some(LatestSettlement {
                state_root: [0xaa; 32],
                permission_root: [0xbb; 32],
                txid: [0xcc; 32],
                daa_score: 789_101,
            })
        );

        let spend = PermissionSpend {
            covenant_id: [0x01; 32],
            old_root: [0xbb; 32],
            old_unclaimed: 2,
            depth: 2,
            leaf_index: 0,
            leaf_spk_bytes: vec![0x02; 32],
            leaf_amount: 50_000_000,
            deduct: 50_000_000,
            new_root: [0xdd; 32],
            spend_txid: [0xee; 32],
            new_outpoint_index: 1,
        };

        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend, &mut wb);
        store.commit(wb);

        let expected_mark =
            SpentMark { spend_txid: [0xee; 32], deduct: 50_000_000, new_root: [0xdd; 32] };
        assert_eq!(leaf_spent(&store, &spend.old_root, 0), Some(expected_mark));
    }
}
