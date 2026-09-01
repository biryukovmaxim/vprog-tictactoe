//! Secondary indexes over the game store: player -> event log (A) and state -> game (B).
//!
//! Pure functions of resource bytes + batch version, executed inside the state WriteBatch
//! (see the spec: docs/internal/specs/2026-08-31-indexer-design.md). Wire decoding goes
//! through the guest lib's codec so the layout has one source of truth.

#![allow(dead_code)]

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

/// B-key codec: `bucket[1] || version_be[8] || game[32]`.
pub fn b_key(bucket: u8, version: u64, game: &ResourceId) -> [u8; 41] {
    let mut key = [0u8; 41];
    key[0] = bucket;
    key[1..9].copy_from_slice(&version.to_be_bytes());
    key[9..41].copy_from_slice(game.as_slice());
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

/// Parses a B-key into `(bucket, version, game)`.
pub fn parse_b_key(key: &[u8]) -> Option<(u8, u64, [u8; 32])> {
    if key.len() != 41 {
        return None;
    }
    let bucket = key[0];
    if !(B_OPEN..=B_FINISHED).contains(&bucket) {
        return None;
    }
    let version = u64::from_be_bytes(key[1..9].try_into().ok()?);
    let game: [u8; 32] = key[9..41].try_into().ok()?;
    Some((bucket, version, game))
}

/// Deletes `game`'s entry (any version) from `bucket`. The end bound appends one byte past
/// the largest possible key of this game in this bucket, a correct half-open end for any
/// game id.
fn clear_bucket(wb: &mut dyn WriteBatch, bucket: u8, game: &ResourceId) {
    let start = b_key(bucket, 0, game);
    let mut end = b_key(bucket, u64::MAX, game).to_vec();
    end.push(0);
    wb.delete_range(StateSpace::Index, &start, &end);
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
            clear_bucket(wb, old_b, id);
        }
        if let Some(new_b) = new_b {
            wb.put(StateSpace::Index, &b_key(new_b, version, id), &[]);
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
            clear_bucket(wb, bucket, id);
        }
        if let Some(g) = game(restored) {
            wb.put(StateSpace::Index, &b_key(bucket_of(g.state()), version, id), &[]);
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

/// Bucket B-scan: `(version, game)` pairs in `bucket`, canonical versions only, newest first.
pub fn scan_bucket<S: Store>(
    store: &S,
    snapshot: &CanonicalChainSnapshot,
    bucket: u8,
) -> Vec<(u64, [u8; 32])> {
    store
        .prefix_iter_rev(StateSpace::Index, &[bucket])
        .filter_map(|(k, _)| {
            let (b, version, game) = parse_b_key(&k)?;
            (b == bucket && snapshot.is_canonical(version)).then_some((version, game))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vprogs_storage_canonical_chain::CanonicalChainManager;
    use vprogs_storage_rocksdb_store::RocksDbStore;

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

        let b = b_key(B_OPEN, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b), Some(vec![]));
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
        wb0.put(StateSpace::Index, &b_key(B_OPEN, 1, &game_id), &[]);
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_events(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        indexer.index_state(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        store.commit(wb);

        // A key for joiner
        let a = a_key(&joiner, TAG_JOINED, 2, &game_id);
        assert_eq!(store.get(StateSpace::Index, &a), Some(vec![]));

        // B: PLAYING key present
        let b_playing = b_key(B_PLAYING, 2, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_playing), Some(vec![]));

        // B: OPEN key cleared
        let b_open = b_key(B_OPEN, 1, &game_id);
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
            wb0.put(StateSpace::Index, &b_key(B_PLAYING, 2, &game_id), &[]);
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

            // B: FINISHED key present
            let b_finished = b_key(B_FINISHED, 3, &game_id);
            assert_eq!(store.get(StateSpace::Index, &b_finished), Some(vec![]));

            // B: old PLAYING key cleared
            let b_playing = b_key(B_PLAYING, 2, &game_id);
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

        // B: FINISHED present, OPEN absent
        let b_finished = b_key(B_FINISHED, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_finished), Some(vec![]));

        let b_open = b_key(B_OPEN, 1, &game_id);
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
        wb0.put(StateSpace::Index, &b_key(B_PLAYING, 2, &game_id), &[]);
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_state(&game_id, Some(&playing1), Some(&playing2), 3, &mut wb);
        store.commit(wb);

        // Old key untouched, no new key at version 3
        let b_old = b_key(B_PLAYING, 2, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_old), Some(vec![]));
        let b_new = b_key(B_PLAYING, 3, &game_id);
        assert_eq!(store.get(StateSpace::Index, &b_new), None);
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
        wb0.put(StateSpace::Index, &b_key(B_FINISHED, 3, &game_id), &[]);
        wb0.put(StateSpace::Index, &b_key(B_OPEN, 1, &game_id), &[]);
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.revert_state(&game_id, Some(&playing), 2, &mut wb);
        store.commit(wb);

        assert_eq!(store.get(StateSpace::Index, &b_key(B_FINISHED, 3, &game_id)), None);
        assert_eq!(store.get(StateSpace::Index, &b_key(B_OPEN, 1, &game_id)), None);
        assert_eq!(store.get(StateSpace::Index, &b_key(B_PLAYING, 2, &game_id)), Some(vec![]));

        // Case 7b: revert to None (game never existed before this fork)
        let mut wb2 = store.write_batch();
        indexer.revert_state(&game_id, None, 0, &mut wb2);
        store.commit(wb2);

        assert_eq!(store.get(StateSpace::Index, &b_key(B_PLAYING, 2, &game_id)), None);
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
            let key = b_key(bucket, version, &game);
            assert_eq!(key.len(), 41);
            let parsed = parse_b_key(&key);
            assert_eq!(parsed, Some((bucket, version, *game)));
        }

        // Invalid lengths
        assert_eq!(parse_a_key(&[0u8; 72]), None);
        assert_eq!(parse_a_key(&[0u8; 74]), None);
        assert_eq!(parse_b_key(&[0u8; 40]), None);
        assert_eq!(parse_b_key(&[0u8; 42]), None);

        // Invalid tag
        let mut invalid_a = a_key(&player, 0, version, &game);
        invalid_a[32] = 0;
        assert_eq!(parse_a_key(&invalid_a), None);
        invalid_a[32] = 6;
        assert_eq!(parse_a_key(&invalid_a), None);

        // Invalid bucket
        let mut invalid_b = b_key(0, version, &game);
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
        wb.put(StateSpace::Index, &b_key(B_OPEN, 1, &game1), &[]);
        wb.put(StateSpace::Index, &b_key(B_OPEN, 2, &game2), &[]);
        wb.put(StateSpace::Index, &b_key(B_OPEN, 3, &game3), &[]);
        store.commit(wb);

        // Scan player events: should return (TAG_CREATED, 1, game1) and (TAG_WON, 3, game3)
        let events = scan_player_events(&store, &snapshot, &player);
        assert_eq!(events, vec![(TAG_CREATED, 1, *game1), (TAG_WON, 3, *game3),]);

        // Scan bucket: should return (3, game3) and (1, game1) in newest-first order
        let bucket_games = scan_bucket(&store, &snapshot, B_OPEN);
        assert_eq!(bucket_games, vec![(3, *game3), (1, *game1),]);
    }
}
