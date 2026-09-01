//! Secondary indexes over the game store: player -> event log (A) and state -> game (B).
//!
//! Pure functions of resource bytes + batch version, executed inside the state WriteBatch
//! (see the spec: docs/internal/specs/2026-08-31-indexer-design.md). Wire decoding goes
//! through the guest lib's codec so the layout has one source of truth.
//!
//! Key and value layouts:
//! - Index A (player event log): `player[32] || tag[1] || version_be[8] || game[32]` (73 bytes),
//!   value empty.
//! - Index B (state bucket): `bucket[1] || game[32]` (33 bytes), value `version_be[8]`.

use vprog_tictactoe_guest::program::resources::game::{GameBody, State};
use vprogs_core_types::ResourceId;
use vprogs_scheduling_scheduler::ResourceIndexer;
use vprogs_storage_canonical_chain::CanonicalChainSnapshot;
use vprogs_storage_types::{StateSpace, Store, WriteBatch};

pub const TAG_CREATED: u8 = 1;
pub const TAG_JOINED: u8 = 2;
pub const TAG_WON: u8 = 3;
pub const TAG_LOST: u8 = 4;
pub const TAG_DRAW: u8 = 5;

pub const B_OPEN: u8 = 0;
pub const B_PLAYING: u8 = 1;
pub const B_FINISHED: u8 = 2;

/// A-key codec: `player[32] || tag[1] || version_be[8] || game[32]`.
pub fn a_key(player: &ResourceId, tag: u8, version: u64, game: &ResourceId) -> [u8; 73] {
    let mut key = [0u8; 73];
    key[0..32].copy_from_slice(player.as_slice());
    key[32] = tag;
    key[33..41].copy_from_slice(&version.to_be_bytes());
    key[41..73].copy_from_slice(game.as_slice());
    key
}

/// B-key codec: `bucket[1] || game[32]`.
pub fn b_key(bucket: u8, game: &ResourceId) -> [u8; 33] {
    let mut key = [0u8; 33];
    key[0] = bucket;
    key[1..33].copy_from_slice(game.as_slice());
    key
}

/// Parses an A-key into `(player, tag, version, game)`.
pub fn parse_a_key(key: &[u8]) -> Option<([u8; 32], u8, u64, [u8; 32])> {
    if key.len() != 73 {
        return None;
    }
    let player: [u8; 32] = key[0..32].try_into().ok()?;
    let tag = key[32];
    if !(TAG_CREATED..=TAG_DRAW).contains(&tag) {
        return None;
    }
    let version = u64::from_be_bytes(key[33..41].try_into().ok()?);
    let game: [u8; 32] = key[41..73].try_into().ok()?;
    Some((player, tag, version, game))
}

/// Parses a B-key into `(bucket, game)`.
pub fn parse_b_key(key: &[u8]) -> Option<(u8, [u8; 32])> {
    if key.len() != 33 {
        return None;
    }
    let bucket = key[0];
    if !(B_OPEN..=B_FINISHED).contains(&bucket) {
        return None;
    }
    let game: [u8; 32] = key[1..33].try_into().ok()?;
    Some((bucket, game))
}

pub struct TicTacToeIndexer;

fn game(bytes: Option<&[u8]>) -> Option<&GameBody> {
    bytes.and_then(|b| GameBody::from_bytes(b).ok())
}

fn bucket_of(state: State) -> u8 {
    match state {
        State::Open => B_OPEN,
        State::Playing => B_PLAYING,
        _ => B_FINISHED,
    }
}

impl ResourceIndexer for TicTacToeIndexer {
    fn index_events(
        &self,
        id: &ResourceId,
        old: Option<&[u8]>,
        new: Option<&[u8]>,
        version: u64,
        wb: &mut dyn WriteBatch,
    ) {
        let (old, new) = (game(old), game(new));
        let Some(new) = new else { return };
        let put = |wb: &mut dyn WriteBatch, player: &ResourceId, tag: u8| {
            wb.put(StateSpace::Index, &a_key(player, tag, version, id), &[]);
        };
        if old.is_none() {
            put(wb, new.creator(), TAG_CREATED);
        }
        if let (None, Some(joiner)) = (old.and_then(|g| g.joiner()), new.joiner()) {
            put(wb, joiner, TAG_JOINED);
        }
        let was_finished = old.is_some_and(|g| g.is_finished());
        if !was_finished && new.is_finished() {
            let (creator_tag, joiner_tag) = match new.state() {
                State::First => (TAG_WON, TAG_LOST),
                State::Second => (TAG_LOST, TAG_WON),
                _ => (TAG_DRAW, TAG_DRAW),
            };
            put(wb, new.creator(), creator_tag);
            if let Some(joiner) = new.joiner() {
                put(wb, joiner, joiner_tag);
            }
        }
    }

    fn index_state(
        &self,
        id: &ResourceId,
        old: Option<&[u8]>,
        new: Option<&[u8]>,
        version: u64,
        wb: &mut dyn WriteBatch,
    ) {
        let (old_b, new_b) =
            (game(old).map(|g| bucket_of(g.state())), game(new).map(|g| bucket_of(g.state())));
        if new_b == old_b {
            return;
        }
        if let Some(old_b) = old_b {
            wb.delete(StateSpace::Index, &b_key(old_b, id));
        }
        if let Some(new_b) = new_b {
            wb.put(StateSpace::Index, &b_key(new_b, id), &version.to_be_bytes());
        }
    }

    fn revert_state(
        &self,
        id: &ResourceId,
        restored: Option<&[u8]>,
        version: u64,
        wb: &mut dyn WriteBatch,
    ) {
        for bucket in [B_OPEN, B_PLAYING, B_FINISHED] {
            wb.delete(StateSpace::Index, &b_key(bucket, id));
        }
        if let Some(g) = game(restored) {
            wb.put(StateSpace::Index, &b_key(bucket_of(g.state()), id), &version.to_be_bytes());
        }
    }
}

/// Player A-scan: `(tag, version, game)` triples for `player`, canonical versions only,
/// version-ascending.
pub fn scan_player_events<S: Store>(
    store: &S,
    snapshot: &CanonicalChainSnapshot,
    player: &ResourceId,
) -> Vec<(u8, u64, [u8; 32])> {
    store
        .prefix_iter(StateSpace::Index, player.as_slice())
        .filter_map(|(k, _)| {
            let (p, tag, version, game) = parse_a_key(&k)?;
            (p == *player.as_slice() && snapshot.is_canonical(version))
                .then_some((tag, version, game))
        })
        .collect()
}

/// Bucket B-scan: `(version, game)` pairs in `bucket`, canonical versions only, sorted newest-first
/// (version descending).
///
/// Ordering is computed in-memory by sorting on the 8-byte version stored in the value,
/// since B-keys are indexed by `bucket || game_id`.
pub fn scan_bucket<S: Store>(
    store: &S,
    snapshot: &CanonicalChainSnapshot,
    bucket: u8,
) -> Vec<(u64, [u8; 32])> {
    let mut results: Vec<(u64, [u8; 32])> = store
        .prefix_iter_rev(StateSpace::Index, &[bucket])
        .filter_map(|(k, v)| {
            let (b, game) = parse_b_key(&k)?;
            if b != bucket || v.len() != 8 {
                return None;
            }
            let version = u64::from_be_bytes(v.as_slice().try_into().ok()?);
            snapshot.is_canonical(version).then_some((version, game))
        })
        .collect();
    results.sort_by_key(|a| std::cmp::Reverse(a.0));
    results
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vprogs_storage_canonical_chain::CanonicalChainManager;
    use vprogs_storage_rocksdb_store::RocksDbStore;

    use super::*;

    fn make_game_bytes(state: State, creator: &ResourceId, joiner: Option<&ResourceId>) -> Vec<u8> {
        let mut bytes = vec![0u8; 108];
        bytes[0] = 2; // Kind::Game
        bytes[1] = state as u8;
        bytes[2] = 1; // Cell::X (creator_mark)
        bytes[3] = 3; // rounds_total
        bytes[23..55].copy_from_slice(creator.as_slice());
        if let Some(j) = joiner {
            bytes[55..87].copy_from_slice(j.as_slice());
        }
        bytes
    }

    fn rid(val: u8) -> ResourceId {
        ResourceId::from([val; 32])
    }

    #[test]
    fn test_case_1_creation() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let game_id = rid(0x30);
        let game_bytes = make_game_bytes(State::Open, &creator, None);

        let mut wb = store.write_batch();
        indexer.index_events(&game_id, None, Some(&game_bytes), 1, &mut wb);
        indexer.index_state(&game_id, None, Some(&game_bytes), 1, &mut wb);
        store.commit(wb);

        let a = a_key(&creator, TAG_CREATED, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, &a), Some(vec![]));

        let b = b_key(B_OPEN, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b), Some(1u64.to_be_bytes().to_vec()));
    }

    #[test]
    fn test_case_2_join() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);

        let open_bytes = make_game_bytes(State::Open, &creator, None);
        let playing_bytes = make_game_bytes(State::Playing, &creator, Some(&joiner));

        // Prior open state in store
        let mut wb0 = store.write_batch();
        wb0.put(StateSpace::Index, &b_key(B_OPEN, &game_id), &1u64.to_be_bytes());
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_events(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        indexer.index_state(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        store.commit(wb);

        // A key for joiner
        let a = a_key(&joiner, TAG_JOINED, 2, &game_id);
        assert_eq!(store.get(StateSpace::Index, &a), Some(vec![]));

        // B: PLAYING key present with version 2
        let b_playing = b_key(B_PLAYING, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_playing), Some(2u64.to_be_bytes().to_vec()));

        // B: OPEN key cleared
        let b_open = b_key(B_OPEN, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_open), None);
    }

    #[test]
    fn test_case_3_finish_variants() {
        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);
        let playing_bytes = make_game_bytes(State::Playing, &creator, Some(&joiner));

        for (state, expected_creator_tag, expected_joiner_tag) in [
            (State::First, TAG_WON, TAG_LOST),
            (State::Second, TAG_LOST, TAG_WON),
            (State::Draw, TAG_DRAW, TAG_DRAW),
        ] {
            let dir = TempDir::new().unwrap();
            let store: RocksDbStore = RocksDbStore::open(dir.path());
            let indexer = TicTacToeIndexer;

            let finish_bytes = make_game_bytes(state, &creator, Some(&joiner));

            // Seed prior playing state in B
            let mut wb0 = store.write_batch();
            wb0.put(StateSpace::Index, &b_key(B_PLAYING, &game_id), &2u64.to_be_bytes());
            store.commit(wb0);

            let mut wb = store.write_batch();
            indexer.index_events(&game_id, Some(&playing_bytes), Some(&finish_bytes), 3, &mut wb);
            indexer.index_state(&game_id, Some(&playing_bytes), Some(&finish_bytes), 3, &mut wb);
            store.commit(wb);

            // A outcome tags
            let a_creator = a_key(&creator, expected_creator_tag, 3, &game_id);
            assert_eq!(store.get(StateSpace::Index, &a_creator), Some(vec![]));

            let a_joiner = a_key(&joiner, expected_joiner_tag, 3, &game_id);
            assert_eq!(store.get(StateSpace::Index, &a_joiner), Some(vec![]));

            // B: FINISHED key present with version 3
            let b_finished = b_key(B_FINISHED, &game_id);
            assert_eq!(
                store.get(StateSpace::Index, &b_finished),
                Some(3u64.to_be_bytes().to_vec())
            );

            // B: old PLAYING key cleared
            let b_playing = b_key(B_PLAYING, &game_id);
            assert_eq!(store.get(StateSpace::Index, &b_playing), None);
        }
    }

    #[test]
    fn test_case_4_same_batch_create_and_finish() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);
        let finished_bytes = make_game_bytes(State::First, &creator, Some(&joiner));

        let mut wb = store.write_batch();
        indexer.index_events(&game_id, None, Some(&finished_bytes), 1, &mut wb);
        indexer.index_state(&game_id, None, Some(&finished_bytes), 1, &mut wb);
        store.commit(wb);

        // Created tag
        let a_created = a_key(&creator, TAG_CREATED, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, &a_created), Some(vec![]));

        // Joined tag
        let a_joined = a_key(&joiner, TAG_JOINED, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, &a_joined), Some(vec![]));

        // Won / Lost tags
        let a_won = a_key(&creator, TAG_WON, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, &a_won), Some(vec![]));
        let a_lost = a_key(&joiner, TAG_LOST, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, &a_lost), Some(vec![]));

        // B: FINISHED present with version 1, OPEN absent
        let b_finished = b_key(B_FINISHED, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_finished), Some(1u64.to_be_bytes().to_vec()));

        let b_open = b_key(B_OPEN, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_open), None);
    }

    #[test]
    fn test_case_5_non_game_kind() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let id = rid(0x30);
        let mut non_game_bytes = vec![0u8; 108];
        non_game_bytes[0] = 1; // Kind::User

        let mut wb = store.write_batch();
        indexer.index_events(&id, None, Some(&non_game_bytes), 1, &mut wb);
        indexer.index_state(&id, None, Some(&non_game_bytes), 1, &mut wb);
        store.commit(wb);

        // No writes should occur in Index CF
        let entries: Vec<_> = store.prefix_iter(StateSpace::Index, &[]).collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_case_6_same_bucket_move() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);

        let playing1 = make_game_bytes(State::Playing, &creator, Some(&joiner));
        let mut playing2 = playing1.clone();
        playing2[15] = 42; // last_move_at changed, but state still Playing

        // Seed store with initial Playing key at version 2
        let mut wb0 = store.write_batch();
        wb0.put(StateSpace::Index, &b_key(B_PLAYING, &game_id), &2u64.to_be_bytes());
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_state(&game_id, Some(&playing1), Some(&playing2), 3, &mut wb);
        store.commit(wb);

        // Key value untouched at version 2
        let b_key = b_key(B_PLAYING, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_key), Some(2u64.to_be_bytes().to_vec()));
    }

    #[test]
    fn test_case_7_revert_state() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);

        // Case 7a: revert to Some(playing)
        let playing = make_game_bytes(State::Playing, &creator, Some(&joiner));
        let mut wb0 = store.write_batch();
        wb0.put(StateSpace::Index, &b_key(B_FINISHED, &game_id), &3u64.to_be_bytes());
        wb0.put(StateSpace::Index, &b_key(B_OPEN, &game_id), &1u64.to_be_bytes());
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.revert_state(&game_id, Some(&playing), 2, &mut wb);
        store.commit(wb);

        assert_eq!(store.get(StateSpace::Index, &b_key(B_FINISHED, &game_id)), None);
        assert_eq!(store.get(StateSpace::Index, &b_key(B_OPEN, &game_id)), None);
        assert_eq!(
            store.get(StateSpace::Index, &b_key(B_PLAYING, &game_id)),
            Some(2u64.to_be_bytes().to_vec())
        );

        // Case 7b: revert to None (game never existed before this fork)
        let mut wb2 = store.write_batch();
        indexer.revert_state(&game_id, None, 0, &mut wb2);
        store.commit(wb2);

        assert_eq!(store.get(StateSpace::Index, &b_key(B_PLAYING, &game_id)), None);
    }

    #[test]
    fn test_case_8_parse_keys_round_trip() {
        let player = rid(7);
        let game = rid(99);
        let version = 1234567890u64;

        for tag in TAG_CREATED..=TAG_DRAW {
            let key = a_key(&player, tag, version, &game);
            assert_eq!(key.len(), 73);
            let parsed = parse_a_key(&key);
            assert_eq!(parsed, Some((*player, tag, version, *game)));
        }

        for bucket in B_OPEN..=B_FINISHED {
            let key = b_key(bucket, &game);
            assert_eq!(key.len(), 33);
            let parsed = parse_b_key(&key);
            assert_eq!(parsed, Some((bucket, *game)));
        }

        // Invalid lengths
        assert_eq!(parse_a_key(&[0u8; 72]), None);
        assert_eq!(parse_a_key(&[0u8; 74]), None);
        assert_eq!(parse_b_key(&[0u8; 32]), None);
        assert_eq!(parse_b_key(&[0u8; 34]), None);

        // Invalid tag
        let mut invalid_a = a_key(&player, 0, version, &game);
        invalid_a[32] = 0;
        assert_eq!(parse_a_key(&invalid_a), None);
        invalid_a[32] = 6;
        assert_eq!(parse_a_key(&invalid_a), None);

        // Invalid bucket
        let mut invalid_b = b_key(0, &game);
        invalid_b[0] = 3;
        assert_eq!(parse_b_key(&invalid_b), None);
    }

    #[test]
    fn test_case_9_canonical_filtered_scans() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());

        let mut manager: CanonicalChainManager<u64> = CanonicalChainManager::default();
        manager.append(1u64);
        manager.append(2u64);
        // Rollback 2, append 3 -> versions 1 and 3 are canonical, version 2 is orphaned
        manager.rollback(1);
        manager.append(3u64);

        let snapshot = manager.chain().snapshot();
        assert!(snapshot.is_canonical(1));
        assert!(!snapshot.is_canonical(2));
        assert!(snapshot.is_canonical(3));

        let player = rid(5);
        let game1 = rid(101);
        let game2 = rid(102);
        let game3 = rid(103);

        let mut wb = store.write_batch();
        // A keys for player
        wb.put(StateSpace::Index, &a_key(&player, TAG_CREATED, 1, &game1), &[]);
        wb.put(StateSpace::Index, &a_key(&player, TAG_CREATED, 2, &game2), &[]);
        wb.put(StateSpace::Index, &a_key(&player, TAG_WON, 3, &game3), &[]);

        // B keys for bucket B_OPEN
        wb.put(StateSpace::Index, &b_key(B_OPEN, &game1), &1u64.to_be_bytes());
        wb.put(StateSpace::Index, &b_key(B_OPEN, &game2), &2u64.to_be_bytes());
        wb.put(StateSpace::Index, &b_key(B_OPEN, &game3), &3u64.to_be_bytes());
        store.commit(wb);

        // Scan player events: should return (TAG_CREATED, 1, game1) and (TAG_WON, 3, game3)
        let events = scan_player_events(&store, &snapshot, &player);
        assert_eq!(events, vec![(TAG_CREATED, 1, *game1), (TAG_WON, 3, *game3),]);

        // Scan bucket: should return (3, game3) and (1, game1) in newest-first order
        let bucket_games = scan_bucket(&store, &snapshot, B_OPEN);
        assert_eq!(bucket_games, vec![(3, *game3), (1, *game1),]);
    }

    #[test]
    fn test_b_transition_spares_other_games() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game1 = rid(0x01);
        let game2 = rid(0x02);

        let open_bytes1 = make_game_bytes(State::Open, &creator, None);
        let playing_bytes1 = make_game_bytes(State::Playing, &creator, Some(&joiner));
        let open_bytes2 = make_game_bytes(State::Open, &creator, None);

        // Seed store with both games in B_OPEN
        let mut wb0 = store.write_batch();
        indexer.index_state(&game1, None, Some(&open_bytes1), 1, &mut wb0);
        indexer.index_state(&game2, None, Some(&open_bytes2), 1, &mut wb0);
        store.commit(wb0);

        assert_eq!(
            store.get(StateSpace::Index, &b_key(B_OPEN, &game1)),
            Some(1u64.to_be_bytes().to_vec())
        );
        assert_eq!(
            store.get(StateSpace::Index, &b_key(B_OPEN, &game2)),
            Some(1u64.to_be_bytes().to_vec())
        );

        // Transition only game 1: Open -> Playing at version 2
        let mut wb = store.write_batch();
        indexer.index_state(&game1, Some(&open_bytes1), Some(&playing_bytes1), 2, &mut wb);
        store.commit(wb);

        // Game 1's old B_OPEN entry is gone, B_PLAYING entry is present
        assert_eq!(store.get(StateSpace::Index, &b_key(B_OPEN, &game1)), None);
        assert_eq!(
            store.get(StateSpace::Index, &b_key(B_PLAYING, &game1)),
            Some(2u64.to_be_bytes().to_vec())
        );

        // Game 2's B_OPEN entry is untouched!
        assert_eq!(
            store.get(StateSpace::Index, &b_key(B_OPEN, &game2)),
            Some(1u64.to_be_bytes().to_vec())
        );
    }

    #[test]
    fn test_b_maintenance_spares_a_keys() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        // Player ID begins with 0x01 (same as B_PLAYING byte)
        let player = rid(0x01);
        let joiner = rid(0x20);
        let game_id = rid(0x01);

        let open_bytes = make_game_bytes(State::Open, &player, None);
        let playing_bytes = make_game_bytes(State::Playing, &player, Some(&joiner));

        // Seed an A key for player and initial B entry
        let mut wb0 = store.write_batch();
        let a_created = a_key(&player, TAG_CREATED, 1, &game_id);
        wb0.put(StateSpace::Index, &a_created, &[]);
        wb0.put(StateSpace::Index, &b_key(B_OPEN, &game_id), &1u64.to_be_bytes());
        store.commit(wb0);

        // Transition B: Open -> Playing
        let mut wb = store.write_batch();
        indexer.index_state(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        store.commit(wb);

        // The A key must survive
        assert_eq!(store.get(StateSpace::Index, &a_created), Some(vec![]));
        // B_OPEN is gone, B_PLAYING is present
        assert_eq!(store.get(StateSpace::Index, &b_key(B_OPEN, &game_id)), None);
        assert_eq!(
            store.get(StateSpace::Index, &b_key(B_PLAYING, &game_id)),
            Some(2u64.to_be_bytes().to_vec())
        );
    }
}
