//! App-side exit-record store tracking settled exit bundles and L1 spent marks.

use std::io::{Read, Write};

use borsh::{BorshDeserialize, BorshSerialize};
use vprogs_runner::ExitLeaf;
use vprogs_storage_types::{StateSpace, Store, WriteBatch};
use vprogs_zk_abi::withdrawal::StandardSpk;
use vprogs_zk_backend_risc0_api::PermissionTreeAccumulator;

/// Discriminator prefix byte for exit records and spent marks in `StateSpace::Index`.
pub const EXITS_DISCRIMINANT: u8 = 0x03;

/// Delimiter byte separating exit root from leaf index in spent mark keys.
pub const SPENT_MARK_DELIMITER: u8 = 0xFF;

/// Reserved marker byte identifying the single latest-settlement key.
pub const LATEST_SETTLEMENT_MARKER: u8 = 0xFE;

/// Record of an on-chain settled exit bundle indexed by its raw padded tree root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitRecord {
    /// L1 transaction id that created the current permission UTXO.
    pub settlement_txid: [u8; 32],
    /// Outpoint index of the permission UTXO in that transaction.
    pub outpoint_index: u32,
    /// DAA score of the block containing the settlement.
    pub daa_score: u64,
    /// Number of unclaimed exits remaining in the family.
    pub unclaimed: u64,
    /// Sompi rent reserved for the continuation UTXO.
    pub rent: u64,
    /// Ordered list of exit leaves in canonical order.
    pub leaves: Vec<ExitLeaf>,
    /// Leaves already claimed on L1, in claim order.
    pub emptied: Vec<EmptiedLeaf>,
}

impl BorshSerialize for ExitRecord {
    fn serialize<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        self.settlement_txid.serialize(writer)?;
        self.outpoint_index.serialize(writer)?;
        self.daa_score.serialize(writer)?;
        self.unclaimed.serialize(writer)?;
        self.rent.serialize(writer)?;
        (self.leaves.len() as u32).serialize(writer)?;
        for leaf in &self.leaves {
            leaf.script_bytes().serialize(writer)?;
            leaf.amount.serialize(writer)?;
        }
        self.emptied.serialize(writer)?;
        Ok(())
    }
}

impl BorshDeserialize for ExitRecord {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let settlement_txid = BorshDeserialize::deserialize_reader(reader)?;
        let outpoint_index = BorshDeserialize::deserialize_reader(reader)?;
        let daa_score = BorshDeserialize::deserialize_reader(reader)?;
        let unclaimed = BorshDeserialize::deserialize_reader(reader)?;
        let rent = BorshDeserialize::deserialize_reader(reader)?;
        let num_leaves = u32::deserialize_reader(reader)?;
        let mut leaves = Vec::with_capacity(num_leaves as usize);
        for _ in 0..num_leaves {
            let script_bytes: Vec<u8> = BorshDeserialize::deserialize_reader(reader)?;
            let amount: u64 = BorshDeserialize::deserialize_reader(reader)?;
            let spk = StandardSpk::from_script(&script_bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            leaves.push(ExitLeaf::from_pair(spk, amount));
        }
        let emptied = BorshDeserialize::deserialize_reader(reader)?;
        Ok(Self { settlement_txid, outpoint_index, daa_score, unclaimed, rent, leaves, emptied })
    }
}

/// One claimed leaf of an exit family, carried in the record's emptied set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EmptiedLeaf {
    /// Position of the claimed leaf in `ExitRecord::leaves`.
    pub leaf_index: u32,
    /// L1 transaction id that claimed the leaf.
    pub spend_txid: [u8; 32],
    /// Sompi amount deducted by the claim.
    pub deduct: u64,
}

/// Marker recording an L1 spend against a specific exit leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SpentMark {
    /// L1 transaction id that spent the leaf.
    pub spend_txid: [u8; 32],
    /// Sompi amount deducted by the claim.
    pub deduct: u64,
    /// Successor permission tree root after this spend.
    pub new_root: [u8; 32],
}

/// Latest paired settlement; a single row overwritten on every committed exit bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct LatestSettlement {
    /// L2 SMT state root after the settled bundle.
    pub state_root: [u8; 32],
    /// Permission SPK hash root of the settled bundle's exit tree.
    pub permission_root: [u8; 32],
    /// L1 transaction id of the settlement.
    pub txid: [u8; 32],
    /// DAA score of the block containing the settlement.
    pub daa_score: u64,
}

/// Materialized Merkle-path view of an exit family's current record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitView {
    /// Current raw padded root of the family (the redeem's `old_root` space).
    pub root: [u8; 32],
    /// Stored exit family record.
    pub record: ExitRecord,
    /// Claimed leaves indexed by leaf position; `None` if unclaimed.
    pub spent: Vec<Option<EmptiedLeaf>>,
    /// Depth of the padded permission tree.
    pub depth: usize,
    /// Sibling hashes over the emptied-folded tree for each leaf, by leaf position.
    pub siblings: Vec<Vec<[u8; 32]>>,
    /// Post-full-claim root for each leaf (its slot emptied too), by leaf position.
    pub full_claim_roots: Vec<[u8; 32]>,
}

/// Builds the 33-byte key for an exit record: `0x03 || root[32]`.
fn exit_record_key(root: &[u8; 32]) -> [u8; 33] {
    let mut key = [0u8; 33];
    key[0] = EXITS_DISCRIMINANT;
    key[1..33].copy_from_slice(root);
    key
}

/// Builds the 38-byte key for a leaf spent mark: `0x03 || root[32] || 0xFF || index_be_u32`.
fn spent_mark_key(root: &[u8; 32], leaf_index: usize) -> [u8; 38] {
    let mut key = [0u8; 38];
    key[0] = EXITS_DISCRIMINANT;
    key[1..33].copy_from_slice(root);
    key[33] = SPENT_MARK_DELIMITER;
    key[34..38].copy_from_slice(&(leaf_index as u32).to_be_bytes());
    key
}

/// Builds the 2-byte key for the latest settlement: `0x03 || 0xFE`.
///
/// The exit keyspace holds records (`0x03 || root[32]`, 33 bytes) and spent marks (`0x03 ||
/// root[32] || 0xFF || index_be_u32`, 38 bytes); this key's reserved marker byte and 2-byte length
/// keep it distinct from both, so `exit_roots` skips it when scanning.
fn latest_settlement_key() -> [u8; 2] {
    [EXITS_DISCRIMINANT, LATEST_SETTLEMENT_MARKER]
}

/// Stores an exit record in the index keyspace under its permission root.
pub fn put_exit_record(wb: &mut dyn WriteBatch, root: &[u8; 32], rec: &ExitRecord) {
    let key = exit_record_key(root);
    let val = borsh::to_vec(rec).expect("ExitRecord borsh serialization should not fail");
    wb.put(StateSpace::Index, &key, &val);
}

/// Reads an exit record by its permission root, or `None` if absent.
pub fn get_exit_record<S: Store>(store: &S, root: &[u8; 32]) -> Option<ExitRecord> {
    let key = exit_record_key(root);
    let val = store.get(StateSpace::Index, &key)?;
    borsh::from_slice(&val).ok()
}

/// Deletes an exit record from the index keyspace.
pub fn delete_exit_record(wb: &mut dyn WriteBatch, root: &[u8; 32]) {
    let key = exit_record_key(root);
    wb.delete(StateSpace::Index, &key);
}

/// Scans all distinct exit roots currently stored in the index.
///
/// Filters strictly for 33-byte record keys (`0x03 || root[32]`), ignoring 38-byte spent marks.
pub fn exit_roots<S: Store>(store: &S) -> Vec<[u8; 32]> {
    let start = [EXITS_DISCRIMINANT];
    let end = [EXITS_DISCRIMINANT + 1];
    store
        .range_iter(StateSpace::Index, &start, &end)
        .filter_map(|(k, _)| {
            if k.len() == 33 && k[0] == EXITS_DISCRIMINANT {
                let mut root = [0u8; 32];
                root.copy_from_slice(&k[1..33]);
                Some(root)
            } else {
                None
            }
        })
        .collect()
}

/// Marks an exit leaf spent in the index keyspace.
pub fn mark_leaf_spent(
    wb: &mut dyn WriteBatch,
    root: &[u8; 32],
    leaf_index: usize,
    mark: &SpentMark,
) {
    let key = spent_mark_key(root, leaf_index);
    let val = borsh::to_vec(mark).expect("SpentMark borsh serialization should not fail");
    wb.put(StateSpace::Index, &key, &val);
}

/// Reads the spent mark for a leaf, or `None` if unspent.
pub fn leaf_spent<S: Store>(store: &S, root: &[u8; 32], leaf_index: usize) -> Option<SpentMark> {
    let key = spent_mark_key(root, leaf_index);
    let val = store.get(StateSpace::Index, &key)?;
    borsh::from_slice(&val).ok()
}

/// Deletes a leaf's spent mark from the index keyspace, leaving the leaf unspent.
///
/// Inverse of `mark_leaf_spent`; idempotent, since deleting an absent key is a no-op.
pub fn unmark_leaf_spent(wb: &mut dyn WriteBatch, root: &[u8; 32], leaf_index: usize) {
    let key = spent_mark_key(root, leaf_index);
    wb.delete(StateSpace::Index, &key);
}

/// Stores the latest settlement, overwriting any previous one.
pub fn put_latest_settlement(wb: &mut dyn WriteBatch, rec: &LatestSettlement) {
    let key = latest_settlement_key();
    let val = borsh::to_vec(rec).expect("LatestSettlement borsh serialization should not fail");
    wb.put(StateSpace::Index, &key, &val);
}

/// Reads the latest settlement, or `None` if no bundle has settled yet.
pub fn latest_settlement<S: Store>(store: &S) -> Option<LatestSettlement> {
    let key = latest_settlement_key();
    let val = store.get(StateSpace::Index, &key)?;
    borsh::from_slice(&val).ok()
}

/// Materializes all stored exit records into Merkle-path views over their emptied-folded trees.
///
/// Sibling paths and per-leaf post-claim roots are computed with the record's emptied slots
/// collapsed to the empty hash (the tree the next claim's redeem verifies against), so they
/// stay valid across a family's whole claim chain. `PermissionTreeView::from_leaves` cannot
/// express emptied slots, so the levels fold directly through the accumulator hashes.
pub fn exit_views<S: Store>(store: &S) -> Vec<ExitView> {
    exit_roots(store)
        .into_iter()
        .filter_map(|root| {
            let record = get_exit_record(store, &root)?;
            let depth = PermissionTreeAccumulator::required_depth(record.leaves.len());
            let empty = PermissionTreeAccumulator::hash_empty();

            // Level 0 over the padded capacity: live leaf hashes, the empty hash for padding
            // and for already-emptied slots.
            let is_emptied = |i: usize| record.emptied.iter().any(|e| e.leaf_index as usize == i);
            let mut level = Vec::with_capacity(1 << depth);
            for i in 0..1usize << depth {
                level.push(if i < record.leaves.len() && !is_emptied(i) {
                    let leaf = &record.leaves[i];
                    PermissionTreeAccumulator::hash_leaf(leaf.to_standard_spk(), leaf.amount)
                } else {
                    empty
                });
            }

            // Branch levels from the leaves up to the root.
            let mut levels = vec![level];
            for _ in 0..depth {
                let prev = levels.last().expect("level built");
                let next = (0..prev.len() / 2)
                    .map(|i| PermissionTreeAccumulator::hash_branch(&prev[2 * i], &prev[2 * i + 1]))
                    .collect();
                levels.push(next);
            }

            let mut spent = vec![None; record.leaves.len()];
            for entry in &record.emptied {
                spent[entry.leaf_index as usize] = Some(*entry);
            }
            let siblings = (0..record.leaves.len())
                .map(|i| {
                    let mut path = Vec::with_capacity(depth);
                    let mut idx = i;
                    for level in &levels[..depth] {
                        path.push(level[idx ^ 1]);
                        idx >>= 1;
                    }
                    path
                })
                .collect();
            let full_claim_roots = (0..record.leaves.len())
                .map(|i| {
                    let mut current = empty;
                    let mut idx = i;
                    for level in &levels[..depth] {
                        current = if idx & 1 == 0 {
                            PermissionTreeAccumulator::hash_branch(&current, &level[idx ^ 1])
                        } else {
                            PermissionTreeAccumulator::hash_branch(&level[idx ^ 1], &current)
                        };
                        idx >>= 1;
                    }
                    current
                })
                .collect();
            Some(ExitView { root, record, spent, depth, siblings, full_claim_roots })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vprogs_storage_rocksdb_store::RocksDbStore;
    use vprogs_zk_backend_risc0_api::{PermissionTreeAccumulator, PermissionTreeView};

    use super::*;

    fn test_record(root_byte: u8) -> ([u8; 32], ExitRecord) {
        let root = [root_byte; 32];
        let pk = [root_byte.wrapping_add(1); 32];
        let leaves = vec![
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk), 50_000_000),
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk), 25_000_000),
        ];
        let rec = ExitRecord {
            settlement_txid: [root_byte.wrapping_add(2); 32],
            outpoint_index: 1,
            daa_score: 123_456,
            unclaimed: 2,
            rent: 50_000_000,
            leaves,
            emptied: Vec::new(),
        };
        (root, rec)
    }

    fn test_spent_mark(spend_byte: u8) -> SpentMark {
        SpentMark {
            spend_txid: [spend_byte; 32],
            deduct: 50_000_000,
            new_root: [spend_byte.wrapping_add(1); 32],
        }
    }

    #[test]
    fn test_record_round_trip_with_emptied_entries() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let (root, mut rec) = test_record(0x11);
        rec.emptied.push(EmptiedLeaf { leaf_index: 0, spend_txid: [0x77; 32], deduct: 50_000_000 });

        let mut wb = store.write_batch();
        put_exit_record(&mut wb, &root, &rec);
        store.commit(wb);

        assert_eq!(get_exit_record(&store, &root), Some(rec));

        let mut wb = store.write_batch();
        delete_exit_record(&mut wb, &root);
        store.commit(wb);
        assert_eq!(get_exit_record(&store, &root), None, "deleted record must vanish");
    }

    #[test]
    fn test_spent_mark_round_trip() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let root = [0x22; 32];
        let mark = test_spent_mark(0x33);

        let mut wb = store.write_batch();
        mark_leaf_spent(&mut wb, &root, 0, &mark);
        store.commit(wb);

        let fetched = leaf_spent(&store, &root, 0);
        assert_eq!(fetched, Some(mark));
    }

    #[test]
    fn test_unmark_leaf_spent_undoes_and_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let root = [0x55; 32];
        let mark = test_spent_mark(0x66);

        let mut wb = store.write_batch();
        mark_leaf_spent(&mut wb, &root, 1, &mark);
        store.commit(wb);
        assert_eq!(leaf_spent(&store, &root, 1), Some(mark));

        let mut wb = store.write_batch();
        unmark_leaf_spent(&mut wb, &root, 1);
        store.commit(wb);
        assert_eq!(leaf_spent(&store, &root, 1), None, "unmarked leaf must read as unspent");

        // Deleting an absent key is a no-op, so a second unmark must not fail.
        let mut wb = store.write_batch();
        unmark_leaf_spent(&mut wb, &root, 1);
        store.commit(wb);
        assert_eq!(leaf_spent(&store, &root, 1), None);
    }

    #[test]
    fn test_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let root = [0x44; 32];

        assert_eq!(get_exit_record(&store, &root), None);
        assert_eq!(leaf_spent(&store, &root, 0), None);
    }

    #[test]
    fn test_exit_roots_lists_distinct_roots_filtering_spent_marks() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());

        let (root1, rec1) = test_record(0x01);
        let (root2, rec2) = test_record(0x02);
        let mark = test_spent_mark(0x09);

        let mut wb = store.write_batch();
        put_exit_record(&mut wb, &root1, &rec1);
        put_exit_record(&mut wb, &root2, &rec2);
        // Insert a spent mark for root1: key starts with 0x03 || root1, but length is 38 bytes.
        mark_leaf_spent(&mut wb, &root1, 0, &mark);
        store.commit(wb);

        let mut roots = exit_roots(&store);
        roots.sort();

        let mut expected = vec![root1, root2];
        expected.sort();

        assert_eq!(roots, expected);
    }

    #[test]
    fn test_exit_views_fold_emptied_slots_into_paths_and_roots() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());

        // Records key on the raw padded root (the redeem's old_root space).
        let pk0 = [0x11u8; 32];
        let pk1 = [0x22u8; 32];
        let leaves1 = vec![
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk0), 50_000_000),
            ExitLeaf::from_pair(StandardSpk::PubKey(&pk1), 25_000_000),
        ];
        let tree1 = PermissionTreeView::from_leaves(&leaves1);
        // Key the advanced family by its continuation root: the fold with leaf 0 emptied.
        let key1 = tree1.root_with_leaf(0, PermissionTreeAccumulator::hash_empty());
        let mut acc = PermissionTreeAccumulator::new();
        for leaf in &leaves1 {
            acc.add_exit(leaf.to_standard_spk(), leaf.amount);
        }
        assert_ne!(acc.finalize(), key1, "raw root must differ from the script-hash commitment");
        let emptied1 = EmptiedLeaf { leaf_index: 0, spend_txid: [0xcc; 32], deduct: 50_000_000 };
        let rec1 = ExitRecord {
            settlement_txid: [0xaa; 32],
            outpoint_index: 1,
            daa_score: 100,
            unclaimed: 1,
            rent: 50_000_000,
            leaves: leaves1,
            emptied: vec![emptied1],
        };

        let pk2 = [0x33u8; 32];
        let leaves2 = vec![ExitLeaf::from_pair(StandardSpk::PubKey(&pk2), 70_000_000)];
        let tree2 = PermissionTreeView::from_leaves(&leaves2);
        let key2 = tree2.root();
        let rec2 = ExitRecord {
            settlement_txid: [0xbb; 32],
            outpoint_index: 1,
            daa_score: 200,
            unclaimed: 1,
            rent: 50_000_000,
            leaves: leaves2,
            emptied: Vec::new(),
        };

        let mut wb = store.write_batch();
        put_exit_record(&mut wb, &key1, &rec1);
        put_exit_record(&mut wb, &key2, &rec2);
        store.commit(wb);

        let views = exit_views(&store);
        assert_eq!(views.len(), 2);

        let empty = PermissionTreeAccumulator::hash_empty();

        // Family 1 after one claim: leaf 0's slot folds to the empty hash, so the view root is
        // the continuation root and leaf 1's sibling is the empty hash, not leaf 0's hash.
        let view1 = views.iter().find(|v| v.root == key1).expect("view1 present");
        assert_eq!(view1.record, rec1);
        assert_eq!(view1.depth, tree1.depth());
        assert_eq!(view1.spent, vec![Some(emptied1), None]);
        let h1 = PermissionTreeAccumulator::hash_leaf(StandardSpk::PubKey(&pk1), 25_000_000);
        assert_eq!(view1.siblings[1], vec![empty]);
        assert_eq!(view1.root, PermissionTreeAccumulator::hash_branch(&empty, &h1));
        assert_eq!(
            view1.full_claim_roots[1],
            PermissionTreeAccumulator::hash_branch(&empty, &empty)
        );

        // Family 2 fresh: unpadded path matches the plain view; claiming the lone leaf empties
        // the whole tree.
        let view2 = views.iter().find(|v| v.root == key2).expect("view2 present");
        assert_eq!(view2.record, rec2);
        assert_eq!(view2.depth, tree2.depth());
        assert_eq!(view2.spent, vec![None]);
        assert_eq!(view2.siblings[0], tree2.siblings(0));
        assert_eq!(
            view2.full_claim_roots[0],
            PermissionTreeAccumulator::hash_branch(&empty, &empty)
        );
    }
}
