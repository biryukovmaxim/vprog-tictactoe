//! Secondary indexes over the game store, maintained by the vprogs write worker inside
//! the same atomic WriteBatch as the state itself (spec:
//! docs/internal/specs/2026-08-31-indexer-design.md).
//!
//! The store is keyed by game resource id only — every query beyond "fetch this exact
//! game" would be a full-store scan. Two indexes close the two queries a frontend
//! actually needs:
//!
//! - **Player event log** ([`PlayerEventKey`]): append-only `player -> (event, version, game)`
//!   facts — created, joined, won, lost, draw. Needed for per-player views ("my games", "games I
//!   won", activity feeds): one prefix scan per player instead of walking every game. Entries are
//!   immutable facts about the version they were written at, so they are never rewritten on reorgs
//!   or prunes; losing-fork entries stay on disk and are filtered out at scan time by canonical
//!   version. `game` is the last key field so a batch that creates *and* finishes a game yields
//!   distinct keys instead of colliding on `(player, version)`.
//! - **Game status index** ([`GameStatusKey`]): current lifecycle bucket per game — open / playing
//!   / finished, exactly one live entry per game, value = the batch version that moved it there.
//!   Needed for lobby views ("open games to join", "recently finished") without reading any game
//!   bodies. Each status transition is an exact delete of the old-bucket key plus a put of the new
//!   one; rollback clears all three bucket keys and re-puts the restored one, so the index always
//!   mirrors the game's latest state.
//!
//! Wire decoding goes through the guest lib's codec so the game-byte layout has one
//! source of truth. Keys are zerocopy structs; their byte layouts are pinned by test.

use vprog_tictactoe_guest::program::resources::game::{GameBody, State};
use vprogs_core_types::ResourceId;
use vprogs_scheduling_scheduler::ResourceIndexer;
use vprogs_storage_canonical_chain::CanonicalChainSnapshot;
use vprogs_storage_types::{StateSpace, Store, WriteBatch};
use zerocopy::{
    Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned, big_endian::U64 as Be64,
};

/// Player-event-log entry kind: one immutable fact per (player, game, version).
/// Discriminants are the on-disk key bytes — append-only, never renumber.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    TryFromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum PlayerEvent {
    /// Game created by this player (creator side).
    Created = 1,
    /// Game joined by this player (joiner side, emitted when the empty joiner seat fills).
    Joined = 2,
    /// Game won by this player at termination.
    Won = 3,
    /// Game lost by this player at termination.
    Lost = 4,
    /// Game drawn (both players get this).
    Draw = 5,
}

/// Game-status bucket: the lifecycle phase a game is currently in. Discriminants are
/// the on-disk key bytes. `Finished` folds the guest's three terminal states
/// (`First`/`Second`/`Draw`) — the winner detail lives in the game body and the
/// player-event log.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    TryFromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum GameStatus {
    /// Waiting for a joiner.
    Open = 0,
    /// Both seats filled, moves in progress.
    Playing = 1,
    /// Terminal (won either way or drawn).
    Finished = 2,
}

/// Player-event-log key: `player[32] || event[1] || version_be[8] || game[32]`
/// (73 bytes, value empty).
#[repr(C)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    TryFromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub struct PlayerEventKey {
    pub player: [u8; 32],
    pub event: PlayerEvent,
    pub version: Be64,
    pub game: [u8; 32],
}

impl PlayerEventKey {
    pub fn new(player: &ResourceId, event: PlayerEvent, version: u64, game: &ResourceId) -> Self {
        Self { player: **player, event, version: Be64::new(version), game: **game }
    }

    /// Parses a raw index key; `None` on wrong length or an invalid event byte.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        Self::try_ref_from_bytes(bytes).ok().copied()
    }
}

/// Game-status key: `status[1] || game[32]` (33 bytes, value `version_be[8]`).
#[repr(C)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    TryFromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub struct GameStatusKey {
    pub status: GameStatus,
    pub game: [u8; 32],
}

impl GameStatusKey {
    pub fn new(status: GameStatus, game: &ResourceId) -> Self {
        Self { status, game: **game }
    }

    /// Parses a raw index key; `None` on wrong length or an invalid status byte.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        Self::try_ref_from_bytes(bytes).ok().copied()
    }
}

pub struct TicTacToeIndexer;

fn game(bytes: Option<&[u8]>) -> Option<&GameBody> {
    bytes.and_then(|b| GameBody::from_bytes(b).ok())
}

fn status_of(state: State) -> GameStatus {
    match state {
        State::Open => GameStatus::Open,
        State::Playing => GameStatus::Playing,
        _ => GameStatus::Finished,
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
        let put = |wb: &mut dyn WriteBatch, player: &ResourceId, event: PlayerEvent| {
            wb.put(
                StateSpace::Index,
                PlayerEventKey::new(player, event, version, id).as_bytes(),
                &[],
            );
        };
        if old.is_none() {
            put(wb, new.creator(), PlayerEvent::Created);
        }
        if let (None, Some(joiner)) = (old.and_then(|g| g.joiner()), new.joiner()) {
            put(wb, joiner, PlayerEvent::Joined);
        }
        let was_finished = old.is_some_and(|g| g.is_finished());
        if !was_finished && new.is_finished() {
            let (creator_event, joiner_event) = match new.state() {
                State::First => (PlayerEvent::Won, PlayerEvent::Lost),
                State::Second => (PlayerEvent::Lost, PlayerEvent::Won),
                _ => (PlayerEvent::Draw, PlayerEvent::Draw),
            };
            put(wb, new.creator(), creator_event);
            if let Some(joiner) = new.joiner() {
                put(wb, joiner, joiner_event);
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
        let (old_s, new_s) =
            (game(old).map(|g| status_of(g.state())), game(new).map(|g| status_of(g.state())));
        if new_s == old_s {
            return;
        }
        if let Some(old_s) = old_s {
            wb.delete(StateSpace::Index, GameStatusKey::new(old_s, id).as_bytes());
        }
        if let Some(new_s) = new_s {
            wb.put(
                StateSpace::Index,
                GameStatusKey::new(new_s, id).as_bytes(),
                &version.to_be_bytes(),
            );
        }
    }

    fn revert_state(
        &self,
        id: &ResourceId,
        restored: Option<&[u8]>,
        version: u64,
        wb: &mut dyn WriteBatch,
    ) {
        for status in [GameStatus::Open, GameStatus::Playing, GameStatus::Finished] {
            wb.delete(StateSpace::Index, GameStatusKey::new(status, id).as_bytes());
        }
        if let Some(g) = game(restored) {
            wb.put(
                StateSpace::Index,
                GameStatusKey::new(status_of(g.state()), id).as_bytes(),
                &version.to_be_bytes(),
            );
        }
    }
}

/// Player-event-log scan: `(event, version, game)` triples for `player`, canonical
/// versions only, version-ascending.
pub fn scan_player_events<S: Store>(
    store: &S,
    snapshot: &CanonicalChainSnapshot,
    player: &ResourceId,
) -> Vec<(PlayerEvent, u64, [u8; 32])> {
    store
        .prefix_iter(StateSpace::Index, player.as_slice())
        .filter_map(|(k, _)| {
            let key = PlayerEventKey::parse(&k)?;
            (key.player == **player && snapshot.is_canonical(key.version.get())).then_some((
                key.event,
                key.version.get(),
                key.game,
            ))
        })
        .collect()
}

/// Game-status scan: `(version, game)` pairs in `status`, canonical versions only,
/// sorted newest-first (version descending).
///
/// Ordering is computed in-memory by sorting on the 8-byte version stored in the
/// value, since keys are ordered by `status || game_id`.
pub fn scan_games_by_status<S: Store>(
    store: &S,
    snapshot: &CanonicalChainSnapshot,
    status: GameStatus,
) -> Vec<(u64, [u8; 32])> {
    let mut results: Vec<(u64, [u8; 32])> = store
        .prefix_iter_rev(StateSpace::Index, status.as_bytes())
        .filter_map(|(k, v)| {
            let key = GameStatusKey::parse(&k)?;
            if v.len() != 8 {
                return None;
            }
            let version = u64::from_be_bytes(v.as_slice().try_into().ok()?);
            snapshot.is_canonical(version).then_some((version, key.game))
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

        let a = PlayerEventKey::new(&creator, PlayerEvent::Created, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, a.as_bytes()), Some(vec![]));

        let b = GameStatusKey::new(GameStatus::Open, &game_id);
        assert_eq!(store.get(StateSpace::Index, b.as_bytes()), Some(1u64.to_be_bytes().to_vec()));
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
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game_id).as_bytes(),
            &1u64.to_be_bytes(),
        );
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_events(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        indexer.index_state(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        store.commit(wb);

        // Player event log: joiner entry
        let a = PlayerEventKey::new(&joiner, PlayerEvent::Joined, 2, &game_id);
        assert_eq!(store.get(StateSpace::Index, a.as_bytes()), Some(vec![]));

        // Status index: Playing present with version 2, Open cleared
        let b_playing = GameStatusKey::new(GameStatus::Playing, &game_id);
        assert_eq!(
            store.get(StateSpace::Index, b_playing.as_bytes()),
            Some(2u64.to_be_bytes().to_vec())
        );

        let b_open = GameStatusKey::new(GameStatus::Open, &game_id);
        assert_eq!(store.get(StateSpace::Index, b_open.as_bytes()), None);
    }

    #[test]
    fn test_case_3_finish_variants() {
        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);
        let playing_bytes = make_game_bytes(State::Playing, &creator, Some(&joiner));

        for (state, expected_creator_event, expected_joiner_event) in [
            (State::First, PlayerEvent::Won, PlayerEvent::Lost),
            (State::Second, PlayerEvent::Lost, PlayerEvent::Won),
            (State::Draw, PlayerEvent::Draw, PlayerEvent::Draw),
        ] {
            let dir = TempDir::new().unwrap();
            let store: RocksDbStore = RocksDbStore::open(dir.path());
            let indexer = TicTacToeIndexer;

            let finish_bytes = make_game_bytes(state, &creator, Some(&joiner));

            // Seed prior playing state
            let mut wb0 = store.write_batch();
            wb0.put(
                StateSpace::Index,
                GameStatusKey::new(GameStatus::Playing, &game_id).as_bytes(),
                &2u64.to_be_bytes(),
            );
            store.commit(wb0);

            let mut wb = store.write_batch();
            indexer.index_events(&game_id, Some(&playing_bytes), Some(&finish_bytes), 3, &mut wb);
            indexer.index_state(&game_id, Some(&playing_bytes), Some(&finish_bytes), 3, &mut wb);
            store.commit(wb);

            // Player event log: outcome entries
            let a_creator = PlayerEventKey::new(&creator, expected_creator_event, 3, &game_id);
            assert_eq!(store.get(StateSpace::Index, a_creator.as_bytes()), Some(vec![]));

            let a_joiner = PlayerEventKey::new(&joiner, expected_joiner_event, 3, &game_id);
            assert_eq!(store.get(StateSpace::Index, a_joiner.as_bytes()), Some(vec![]));

            // Status index: Finished present with version 3, Playing cleared
            let b_finished = GameStatusKey::new(GameStatus::Finished, &game_id);
            assert_eq!(
                store.get(StateSpace::Index, b_finished.as_bytes()),
                Some(3u64.to_be_bytes().to_vec())
            );

            let b_playing = GameStatusKey::new(GameStatus::Playing, &game_id);
            assert_eq!(store.get(StateSpace::Index, b_playing.as_bytes()), None);
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

        // Created / Joined / Won / Lost entries
        for (player, event) in [
            (&creator, PlayerEvent::Created),
            (&joiner, PlayerEvent::Joined),
            (&creator, PlayerEvent::Won),
            (&joiner, PlayerEvent::Lost),
        ] {
            let key = PlayerEventKey::new(player, event, 1, &game_id);
            assert_eq!(store.get(StateSpace::Index, key.as_bytes()), Some(vec![]));
        }

        // Status index: Finished present with version 1, Open absent
        let b_finished = GameStatusKey::new(GameStatus::Finished, &game_id);
        assert_eq!(
            store.get(StateSpace::Index, b_finished.as_bytes()),
            Some(1u64.to_be_bytes().to_vec())
        );

        let b_open = GameStatusKey::new(GameStatus::Open, &game_id);
        assert_eq!(store.get(StateSpace::Index, b_open.as_bytes()), None);
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
    fn test_case_6_same_status_move() {
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
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Playing, &game_id).as_bytes(),
            &2u64.to_be_bytes(),
        );
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_state(&game_id, Some(&playing1), Some(&playing2), 3, &mut wb);
        store.commit(wb);

        // Key value untouched at version 2
        let b_key = GameStatusKey::new(GameStatus::Playing, &game_id);
        assert_eq!(
            store.get(StateSpace::Index, b_key.as_bytes()),
            Some(2u64.to_be_bytes().to_vec())
        );
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
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Finished, &game_id).as_bytes(),
            &3u64.to_be_bytes(),
        );
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game_id).as_bytes(),
            &1u64.to_be_bytes(),
        );
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.revert_state(&game_id, Some(&playing), 2, &mut wb);
        store.commit(wb);

        let get = |status: GameStatus| {
            store.get(StateSpace::Index, GameStatusKey::new(status, &game_id).as_bytes())
        };
        assert_eq!(get(GameStatus::Finished), None);
        assert_eq!(get(GameStatus::Open), None);
        assert_eq!(get(GameStatus::Playing), Some(2u64.to_be_bytes().to_vec()));

        // Case 7b: revert to None (game never existed before this fork)
        let mut wb2 = store.write_batch();
        indexer.revert_state(&game_id, None, 0, &mut wb2);
        store.commit(wb2);

        assert_eq!(get(GameStatus::Playing), None);
    }

    #[test]
    fn test_case_8_parse_keys_round_trip() {
        let player = rid(7);
        let game = rid(99);
        let version = 1234567890u64;

        for event in [
            PlayerEvent::Created,
            PlayerEvent::Joined,
            PlayerEvent::Won,
            PlayerEvent::Lost,
            PlayerEvent::Draw,
        ] {
            let key = PlayerEventKey::new(&player, event, version, &game);
            assert_eq!(key.as_bytes().len(), 73);
            assert_eq!(PlayerEventKey::parse(key.as_bytes()), Some(key));
        }

        for status in [GameStatus::Open, GameStatus::Playing, GameStatus::Finished] {
            let key = GameStatusKey::new(status, &game);
            assert_eq!(key.as_bytes().len(), 33);
            assert_eq!(GameStatusKey::parse(key.as_bytes()), Some(key));
        }

        // Wire layouts are pinned to the documented byte offsets.
        let a = PlayerEventKey::new(&player, PlayerEvent::Won, version, &game);
        assert_eq!(&a.as_bytes()[0..32], player.as_slice());
        assert_eq!(a.as_bytes()[32], PlayerEvent::Won as u8);
        assert_eq!(&a.as_bytes()[33..41], &version.to_be_bytes()[..]);
        assert_eq!(&a.as_bytes()[41..73], game.as_slice());

        let b = GameStatusKey::new(GameStatus::Playing, &game);
        assert_eq!(b.as_bytes()[0], GameStatus::Playing as u8);
        assert_eq!(&b.as_bytes()[1..33], game.as_slice());

        // Invalid lengths
        assert_eq!(PlayerEventKey::parse(&[0u8; 72]), None);
        assert_eq!(PlayerEventKey::parse(&[0u8; 74]), None);
        assert_eq!(GameStatusKey::parse(&[0u8; 32]), None);
        assert_eq!(GameStatusKey::parse(&[0u8; 34]), None);

        // Invalid event byte
        let mut invalid_a = a.as_bytes().to_vec();
        invalid_a[32] = 0;
        assert_eq!(PlayerEventKey::parse(&invalid_a), None);
        invalid_a[32] = 6;
        assert_eq!(PlayerEventKey::parse(&invalid_a), None);

        // Invalid status byte
        let mut invalid_b = b.as_bytes().to_vec();
        invalid_b[0] = 3;
        assert_eq!(GameStatusKey::parse(&invalid_b), None);
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
        // Player-event-log keys
        wb.put(
            StateSpace::Index,
            PlayerEventKey::new(&player, PlayerEvent::Created, 1, &game1).as_bytes(),
            &[],
        );
        wb.put(
            StateSpace::Index,
            PlayerEventKey::new(&player, PlayerEvent::Created, 2, &game2).as_bytes(),
            &[],
        );
        wb.put(
            StateSpace::Index,
            PlayerEventKey::new(&player, PlayerEvent::Won, 3, &game3).as_bytes(),
            &[],
        );

        // Status keys in Open
        wb.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game1).as_bytes(),
            &1u64.to_be_bytes(),
        );
        wb.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game2).as_bytes(),
            &2u64.to_be_bytes(),
        );
        wb.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game3).as_bytes(),
            &3u64.to_be_bytes(),
        );
        store.commit(wb);

        // Player-event scan: (Created, 1, game1) and (Won, 3, game3)
        let events = scan_player_events(&store, &snapshot, &player);
        assert_eq!(events, vec![(PlayerEvent::Created, 1, *game1), (PlayerEvent::Won, 3, *game3),]);

        // Status scan: (3, game3) and (1, game1), newest-first
        let open_games = scan_games_by_status(&store, &snapshot, GameStatus::Open);
        assert_eq!(open_games, vec![(3, *game3), (1, *game1),]);
    }

    #[test]
    fn test_status_transition_spares_other_games() {
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

        // Seed store with both games in Open
        let mut wb0 = store.write_batch();
        indexer.index_state(&game1, None, Some(&open_bytes1), 1, &mut wb0);
        indexer.index_state(&game2, None, Some(&open_bytes2), 1, &mut wb0);
        store.commit(wb0);

        let get = |status: GameStatus, game: &ResourceId| {
            store.get(StateSpace::Index, GameStatusKey::new(status, game).as_bytes())
        };
        assert_eq!(get(GameStatus::Open, &game1), Some(1u64.to_be_bytes().to_vec()));
        assert_eq!(get(GameStatus::Open, &game2), Some(1u64.to_be_bytes().to_vec()));

        // Transition only game 1: Open -> Playing at version 2
        let mut wb = store.write_batch();
        indexer.index_state(&game1, Some(&open_bytes1), Some(&playing_bytes1), 2, &mut wb);
        store.commit(wb);

        // Game 1's Open entry is gone, Playing entry is present
        assert_eq!(get(GameStatus::Open, &game1), None);
        assert_eq!(get(GameStatus::Playing, &game1), Some(2u64.to_be_bytes().to_vec()));

        // Game 2's Open entry is untouched!
        assert_eq!(get(GameStatus::Open, &game2), Some(1u64.to_be_bytes().to_vec()));
    }

    #[test]
    fn test_status_maintenance_spares_event_log_keys() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        // Player ID begins with 0x01 (same as the Playing status byte)
        let player = rid(0x01);
        let joiner = rid(0x20);
        let game_id = rid(0x01);

        let open_bytes = make_game_bytes(State::Open, &player, None);
        let playing_bytes = make_game_bytes(State::Playing, &player, Some(&joiner));

        // Seed an event-log key for player and initial status entry
        let mut wb0 = store.write_batch();
        let a_created = PlayerEventKey::new(&player, PlayerEvent::Created, 1, &game_id);
        wb0.put(StateSpace::Index, a_created.as_bytes(), &[]);
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game_id).as_bytes(),
            &1u64.to_be_bytes(),
        );
        store.commit(wb0);

        // Transition: Open -> Playing
        let mut wb = store.write_batch();
        indexer.index_state(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        store.commit(wb);

        // The event-log key must survive
        assert_eq!(store.get(StateSpace::Index, a_created.as_bytes()), Some(vec![]));
        // Open is gone, Playing is present
        assert_eq!(
            store.get(StateSpace::Index, GameStatusKey::new(GameStatus::Open, &game_id).as_bytes()),
            None
        );
        assert_eq!(
            store.get(
                StateSpace::Index,
                GameStatusKey::new(GameStatus::Playing, &game_id).as_bytes()
            ),
            Some(2u64.to_be_bytes().to_vec())
        );
    }
}
