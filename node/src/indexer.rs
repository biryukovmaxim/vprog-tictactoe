//! Secondary indexes over the game store, so common queries don't scan every game:
//!
//! - **Player event log** ([`PlayerEventKey`]): append-only per-player facts (created, joined, won,
//!   lost, drawn) for "my games" style views.
//! - **Game status index** ([`GameStatusKey`]): one live entry per game in its current bucket
//!   (open, playing, finished) for lobby and recent-finish views.

use vprog_tictactoe_guest::program::resources::game::{GameBody, State};
use vprogs_core_types::ResourceId;
use vprogs_scheduling_scheduler::ResourceIndexer;
use vprogs_storage_canonical_chain::CanonicalChainSnapshot;
use vprogs_storage_types::{StateSpace, Store, WriteBatch};
use zerocopy::{
    Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned, big_endian::U64 as Be64,
};

/// Discriminator prefix byte for player-event-log keys.
pub const PLAYER_EVENT_DISCRIMINATOR: u8 = 0x01;

/// Discriminator prefix byte for game-status keys.
pub const GAME_STATUS_DISCRIMINATOR: u8 = 0x02;

/// Player-event-log entry kind: one immutable fact per (player, game, version).
/// Discriminants are the on-disk key bytes; append-only, never renumber.
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
    /// Game created by this player.
    Created = 1,
    /// Game joined by this player.
    Joined = 2,
    /// Game won by this player.
    Won = 3,
    /// Game lost by this player.
    Lost = 4,
    /// Game drawn by this player.
    Draw = 5,
}

/// Game-status bucket: the game's current lifecycle phase. Discriminants are the
/// on-disk key bytes. `Finished` covers all terminal states; the winner detail lives
/// in the game body and the player-event log.
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
    /// Moves in progress.
    Playing = 1,
    /// Terminal: won or drawn.
    Finished = 2,
}

/// Player-event-log key: `0x01 || player[32] || event[1] || version_be[8] || game[32]`;
/// the value is empty.
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
    pub discriminator: u8,
    pub player: [u8; 32],
    pub event: PlayerEvent,
    pub version: Be64,
    pub game: [u8; 32],
}

impl PlayerEventKey {
    /// Creates a key from its parts.
    pub fn new(player: &ResourceId, event: PlayerEvent, version: u64, game: &ResourceId) -> Self {
        Self {
            discriminator: PLAYER_EVENT_DISCRIMINATOR,
            player: **player,
            event,
            version: Be64::new(version),
            game: **game,
        }
    }

    /// Parses a raw index key; `None` on wrong length, invalid discriminator, or invalid event.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let key = Self::try_ref_from_bytes(bytes).ok().copied()?;
        (key.discriminator == PLAYER_EVENT_DISCRIMINATOR).then_some(key)
    }
}

/// Game-status key: `0x02 || status[1] || game[32]`; the value is the version that wrote it.
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
    pub discriminator: u8,
    pub status: GameStatus,
    pub game: [u8; 32],
}

impl GameStatusKey {
    /// Creates a key from its parts.
    pub fn new(status: GameStatus, game: &ResourceId) -> Self {
        Self { discriminator: GAME_STATUS_DISCRIMINATOR, status, game: **game }
    }

    /// Parses a raw index key; `None` on wrong length, invalid discriminator, or invalid status.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let key = Self::try_ref_from_bytes(bytes).ok().copied()?;
        (key.discriminator == GAME_STATUS_DISCRIMINATOR).then_some(key)
    }
}

/// Maintains both indexes as game resources change.
pub struct TicTacToeIndexer;

/// Decodes an optional raw game body; `None` for non-game bytes.
fn game(bytes: Option<&[u8]>) -> Option<&GameBody> {
    bytes.and_then(|b| GameBody::from_bytes(b).ok())
}

/// Maps a game state to its status bucket.
fn status_of(state: State) -> GameStatus {
    match state {
        State::Open => GameStatus::Open,
        State::Playing => GameStatus::Playing,
        _ => GameStatus::Finished,
    }
}

/// Pure helper extracting forward player events emitted across an old-to-new game transition.
fn forward_events<'a>(
    old: Option<&'a GameBody>,
    new: Option<&'a GameBody>,
) -> impl Iterator<Item = (&'a ResourceId, PlayerEvent)> {
    let mut events = Vec::with_capacity(4);
    let Some(new) = new else { return events.into_iter() };
    if old.is_none() {
        events.push((new.creator(), PlayerEvent::Created));
    }
    if let (None, Some(joiner)) = (old.and_then(|g| g.joiner()), new.joiner()) {
        events.push((joiner, PlayerEvent::Joined));
    }
    let was_finished = old.is_some_and(|g| g.is_finished());
    if !was_finished && new.is_finished() {
        let (creator_event, joiner_event) = match new.state() {
            State::First => (PlayerEvent::Won, PlayerEvent::Lost),
            State::Second => (PlayerEvent::Lost, PlayerEvent::Won),
            _ => (PlayerEvent::Draw, PlayerEvent::Draw),
        };
        events.push((new.creator(), creator_event));
        if let Some(joiner) = new.joiner() {
            events.push((joiner, joiner_event));
        }
    }
    events.into_iter()
}

impl ResourceIndexer for TicTacToeIndexer {
    fn index_diff(
        &self,
        id: &ResourceId,
        old: Option<&[u8]>,
        new: Option<&[u8]>,
        version: u64,
        wb: &mut dyn WriteBatch,
    ) {
        let (old_g, new_g) = (game(old), game(new));

        // Player event log: forward player events.
        for (player, event) in forward_events(old_g, new_g) {
            wb.put(
                StateSpace::Index,
                PlayerEventKey::new(player, event, version, id).as_bytes(),
                &[],
            );
        }

        // Game status index: move the game to its new bucket on status change.
        let old_s = old_g.map(|g| status_of(g.state()));
        let new_s = new_g.map(|g| status_of(g.state()));
        if new_s != old_s {
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
    }

    fn revert_diff(
        &self,
        id: &ResourceId,
        written: Option<&[u8]>,
        restored: Option<&[u8]>,
        reverted_version: u64,
        restored_version: u64,
        wb: &mut dyn WriteBatch,
    ) {
        let (written_g, restored_g) = (game(written), game(restored));

        // Player event log: delete entries the reverted diff inserted at reverted_version.
        for (player, event) in forward_events(restored_g, written_g) {
            wb.delete(
                StateSpace::Index,
                PlayerEventKey::new(player, event, reverted_version, id).as_bytes(),
            );
        }

        // Game status index: clear all status keys and restore the prior status if any.
        for status in [GameStatus::Open, GameStatus::Playing, GameStatus::Finished] {
            wb.delete(StateSpace::Index, GameStatusKey::new(status, id).as_bytes());
        }
        if let Some(g) = restored_g {
            wb.put(
                StateSpace::Index,
                GameStatusKey::new(status_of(g.state()), id).as_bytes(),
                &restored_version.to_be_bytes(),
            );
        }
    }
}

/// Event stream for one (player, event): (version, game), ascending,
/// canonical versions only. `after` is an exclusive cursor (the last entry
/// seen); `limit` bounds the page.
pub fn scan_player_events<S: Store>(
    store: &S,
    snapshot: &CanonicalChainSnapshot,
    player: &ResourceId,
    event: PlayerEvent,
    after: Option<(u64, [u8; 32])>,
    limit: usize,
) -> Vec<(u64, [u8; 32])> {
    if limit == 0 {
        return Vec::new();
    }
    let (start, end) = match after {
        Some((version, game)) => {
            let key = PlayerEventKey {
                discriminator: PLAYER_EVENT_DISCRIMINATOR,
                player: **player,
                event,
                version: Be64::new(version),
                game,
            };
            let mut start = Vec::with_capacity(75);
            start.extend_from_slice(key.as_bytes());
            start.push(0x00);
            let mut end = Vec::with_capacity(34);
            end.push(PLAYER_EVENT_DISCRIMINATOR);
            end.extend_from_slice(player.as_slice());
            end.push(event as u8 + 1);
            (start, end)
        }
        None => {
            let mut start = Vec::with_capacity(42);
            start.push(PLAYER_EVENT_DISCRIMINATOR);
            start.extend_from_slice(player.as_slice());
            start.push(event as u8);
            start.extend_from_slice(&0u64.to_be_bytes());
            let mut end = Vec::with_capacity(34);
            end.push(PLAYER_EVENT_DISCRIMINATOR);
            end.extend_from_slice(player.as_slice());
            end.push(event as u8 + 1);
            (start, end)
        }
    };

    store
        .range_iter(StateSpace::Index, &start, &end)
        .filter_map(|(k, _)| {
            let key = PlayerEventKey::parse(&k)
                .unwrap_or_else(|| panic!("malformed player-event key in index: {k:02x?}"));
            snapshot.is_canonical(key.version.get()).then_some((key.version.get(), key.game))
        })
        .take(limit)
        .collect()
}

/// Games in `status`, game-id ascending, canonical versions only.
/// `after_game` is an exclusive cursor; `limit` bounds the page. Games
/// created after the scan started may be missed until the caller restarts
/// from the beginning (accepted).
pub fn scan_games_by_status<S: Store>(
    store: &S,
    snapshot: &CanonicalChainSnapshot,
    status: GameStatus,
    after_game: Option<ResourceId>,
    limit: usize,
) -> Vec<[u8; 32]> {
    if limit == 0 {
        return Vec::new();
    }
    let (start, end) = match after_game {
        Some(game) => {
            let key = GameStatusKey::new(status, &game);
            let mut start = Vec::with_capacity(35);
            start.extend_from_slice(key.as_bytes());
            start.push(0x00);
            let end = [GAME_STATUS_DISCRIMINATOR, status as u8 + 1];
            (start, end.to_vec())
        }
        None => {
            let start = [GAME_STATUS_DISCRIMINATOR, status as u8];
            let end = [GAME_STATUS_DISCRIMINATOR, status as u8 + 1];
            (start.to_vec(), end.to_vec())
        }
    };

    store
        .range_iter(StateSpace::Index, &start, &end)
        .filter_map(|(k, v)| {
            let key = GameStatusKey::parse(&k)
                .unwrap_or_else(|| panic!("malformed game-status key in index: {k:02x?}"));
            let version = u64::from_be_bytes(
                v.as_slice()
                    .try_into()
                    .unwrap_or_else(|_| panic!("malformed game-status value in index: {v:02x?}")),
            );
            snapshot.is_canonical(version).then_some(key.game)
        })
        .take(limit)
        .collect()
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
        indexer.index_diff(&game_id, None, Some(&game_bytes), 1, &mut wb);
        store.commit(wb);

        let created_key = PlayerEventKey::new(&creator, PlayerEvent::Created, 1, &game_id);
        assert_eq!(store.get(StateSpace::Index, created_key.as_bytes()), Some(vec![]));

        let status_open = GameStatusKey::new(GameStatus::Open, &game_id);
        assert_eq!(
            store.get(StateSpace::Index, status_open.as_bytes()),
            Some(1u64.to_be_bytes().to_vec())
        );
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

        // Prior open state in store.
        let mut wb0 = store.write_batch();
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game_id).as_bytes(),
            &1u64.to_be_bytes(),
        );
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_diff(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        store.commit(wb);

        // Player event log: joiner entry.
        let joined_key = PlayerEventKey::new(&joiner, PlayerEvent::Joined, 2, &game_id);
        assert_eq!(store.get(StateSpace::Index, joined_key.as_bytes()), Some(vec![]));

        // Status index: Playing present with version 2, Open cleared.
        let status_playing = GameStatusKey::new(GameStatus::Playing, &game_id);
        assert_eq!(
            store.get(StateSpace::Index, status_playing.as_bytes()),
            Some(2u64.to_be_bytes().to_vec())
        );

        let status_open = GameStatusKey::new(GameStatus::Open, &game_id);
        assert_eq!(store.get(StateSpace::Index, status_open.as_bytes()), None);
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

            // Seed prior playing state.
            let mut wb0 = store.write_batch();
            wb0.put(
                StateSpace::Index,
                GameStatusKey::new(GameStatus::Playing, &game_id).as_bytes(),
                &2u64.to_be_bytes(),
            );
            store.commit(wb0);

            let mut wb = store.write_batch();
            indexer.index_diff(&game_id, Some(&playing_bytes), Some(&finish_bytes), 3, &mut wb);
            store.commit(wb);

            // Player event log: outcome entries.
            let creator_event_key =
                PlayerEventKey::new(&creator, expected_creator_event, 3, &game_id);
            assert_eq!(store.get(StateSpace::Index, creator_event_key.as_bytes()), Some(vec![]));

            let joiner_event_key = PlayerEventKey::new(&joiner, expected_joiner_event, 3, &game_id);
            assert_eq!(store.get(StateSpace::Index, joiner_event_key.as_bytes()), Some(vec![]));

            // Status index: Finished present with version 3, Playing cleared.
            let status_finished = GameStatusKey::new(GameStatus::Finished, &game_id);
            assert_eq!(
                store.get(StateSpace::Index, status_finished.as_bytes()),
                Some(3u64.to_be_bytes().to_vec())
            );

            let status_playing = GameStatusKey::new(GameStatus::Playing, &game_id);
            assert_eq!(store.get(StateSpace::Index, status_playing.as_bytes()), None);
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
        indexer.index_diff(&game_id, None, Some(&finished_bytes), 1, &mut wb);
        store.commit(wb);

        // Created / Joined / Won / Lost entries.
        for (player, event) in [
            (&creator, PlayerEvent::Created),
            (&joiner, PlayerEvent::Joined),
            (&creator, PlayerEvent::Won),
            (&joiner, PlayerEvent::Lost),
        ] {
            let key = PlayerEventKey::new(player, event, 1, &game_id);
            assert_eq!(store.get(StateSpace::Index, key.as_bytes()), Some(vec![]));
        }

        // Status index: Finished present with version 1, Open absent.
        let status_finished = GameStatusKey::new(GameStatus::Finished, &game_id);
        assert_eq!(
            store.get(StateSpace::Index, status_finished.as_bytes()),
            Some(1u64.to_be_bytes().to_vec())
        );

        let status_open = GameStatusKey::new(GameStatus::Open, &game_id);
        assert_eq!(store.get(StateSpace::Index, status_open.as_bytes()), None);
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
        indexer.index_diff(&id, None, Some(&non_game_bytes), 1, &mut wb);
        store.commit(wb);

        // No writes should occur in Index CF.
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

        // Seed store with initial Playing key at version 2.
        let mut wb0 = store.write_batch();
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Playing, &game_id).as_bytes(),
            &2u64.to_be_bytes(),
        );
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.index_diff(&game_id, Some(&playing1), Some(&playing2), 3, &mut wb);
        store.commit(wb);

        // Key value untouched at version 2.
        let status_key = GameStatusKey::new(GameStatus::Playing, &game_id);
        assert_eq!(
            store.get(StateSpace::Index, status_key.as_bytes()),
            Some(2u64.to_be_bytes().to_vec())
        );
    }

    #[test]
    fn test_case_7_revert_diff() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);

        // Case 7a: revert Playing -> Finished at V3 back to Playing at V2.
        let playing = make_game_bytes(State::Playing, &creator, Some(&joiner));
        let finished = make_game_bytes(State::First, &creator, Some(&joiner));

        let mut wb0 = store.write_batch();
        // Seed V1 Created, V2 Joined, V3 Won/Lost
        wb0.put(
            StateSpace::Index,
            PlayerEventKey::new(&creator, PlayerEvent::Created, 1, &game_id).as_bytes(),
            &[],
        );
        wb0.put(
            StateSpace::Index,
            PlayerEventKey::new(&joiner, PlayerEvent::Joined, 2, &game_id).as_bytes(),
            &[],
        );
        wb0.put(
            StateSpace::Index,
            PlayerEventKey::new(&creator, PlayerEvent::Won, 3, &game_id).as_bytes(),
            &[],
        );
        wb0.put(
            StateSpace::Index,
            PlayerEventKey::new(&joiner, PlayerEvent::Lost, 3, &game_id).as_bytes(),
            &[],
        );
        // Seed Finished at V3
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Finished, &game_id).as_bytes(),
            &3u64.to_be_bytes(),
        );
        store.commit(wb0);

        let mut wb = store.write_batch();
        indexer.revert_diff(&game_id, Some(&finished), Some(&playing), 3, 2, &mut wb);
        store.commit(wb);

        // Won@3 and Lost@3 deleted; Created@1 and Joined@2 preserved.
        assert_eq!(
            store.get(
                StateSpace::Index,
                PlayerEventKey::new(&creator, PlayerEvent::Won, 3, &game_id).as_bytes()
            ),
            None
        );
        assert_eq!(
            store.get(
                StateSpace::Index,
                PlayerEventKey::new(&joiner, PlayerEvent::Lost, 3, &game_id).as_bytes()
            ),
            None
        );
        assert_eq!(
            store.get(
                StateSpace::Index,
                PlayerEventKey::new(&creator, PlayerEvent::Created, 1, &game_id).as_bytes()
            ),
            Some(vec![])
        );
        assert_eq!(
            store.get(
                StateSpace::Index,
                PlayerEventKey::new(&joiner, PlayerEvent::Joined, 2, &game_id).as_bytes()
            ),
            Some(vec![])
        );

        let get_status = |status: GameStatus| {
            store.get(StateSpace::Index, GameStatusKey::new(status, &game_id).as_bytes())
        };
        assert_eq!(get_status(GameStatus::Finished), None);
        assert_eq!(get_status(GameStatus::Open), None);
        assert_eq!(get_status(GameStatus::Playing), Some(2u64.to_be_bytes().to_vec()));

        // Case 7b: revert to None (game created at V1, reverted before genesis).
        let open = make_game_bytes(State::Open, &creator, None);
        let mut wb2 = store.write_batch();
        indexer.revert_diff(&game_id, Some(&open), None, 1, 0, &mut wb2);
        store.commit(wb2);

        assert_eq!(
            store.get(
                StateSpace::Index,
                PlayerEventKey::new(&creator, PlayerEvent::Created, 1, &game_id).as_bytes()
            ),
            None
        );
        assert_eq!(get_status(GameStatus::Playing), None);
        assert_eq!(get_status(GameStatus::Open), None);
    }

    #[test]
    fn test_forward_events_same_batch_create_and_finish_revert() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        let creator = rid(0x10);
        let joiner = rid(0x20);
        let game_id = rid(0x30);
        let finished = make_game_bytes(State::First, &creator, Some(&joiner));

        // Commit same-batch create and finish at V1.
        let mut wb = store.write_batch();
        indexer.index_diff(&game_id, None, Some(&finished), 1, &mut wb);
        store.commit(wb);

        // All 4 events present.
        for (player, event) in [
            (&creator, PlayerEvent::Created),
            (&joiner, PlayerEvent::Joined),
            (&creator, PlayerEvent::Won),
            (&joiner, PlayerEvent::Lost),
        ] {
            assert_eq!(
                store.get(
                    StateSpace::Index,
                    PlayerEventKey::new(player, event, 1, &game_id).as_bytes()
                ),
                Some(vec![])
            );
        }
        assert_eq!(
            store.get(
                StateSpace::Index,
                GameStatusKey::new(GameStatus::Finished, &game_id).as_bytes()
            ),
            Some(1u64.to_be_bytes().to_vec())
        );

        // Revert V1 back to genesis (None).
        let mut wb_rev = store.write_batch();
        indexer.revert_diff(&game_id, Some(&finished), None, 1, 0, &mut wb_rev);
        store.commit(wb_rev);

        // All 4 events and Finished status deleted.
        for (player, event) in [
            (&creator, PlayerEvent::Created),
            (&joiner, PlayerEvent::Joined),
            (&creator, PlayerEvent::Won),
            (&joiner, PlayerEvent::Lost),
        ] {
            assert_eq!(
                store.get(
                    StateSpace::Index,
                    PlayerEventKey::new(player, event, 1, &game_id).as_bytes()
                ),
                None
            );
        }
        assert_eq!(
            store.get(
                StateSpace::Index,
                GameStatusKey::new(GameStatus::Finished, &game_id).as_bytes()
            ),
            None
        );
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
            assert_eq!(key.as_bytes().len(), 74);
            assert_eq!(PlayerEventKey::parse(key.as_bytes()), Some(key));
        }

        for status in [GameStatus::Open, GameStatus::Playing, GameStatus::Finished] {
            let key = GameStatusKey::new(status, &game);
            assert_eq!(key.as_bytes().len(), 34);
            assert_eq!(GameStatusKey::parse(key.as_bytes()), Some(key));
        }

        // Wire layouts are pinned to the documented byte offsets.
        let event_key = PlayerEventKey::new(&player, PlayerEvent::Won, version, &game);
        assert_eq!(event_key.as_bytes()[0], PLAYER_EVENT_DISCRIMINATOR);
        assert_eq!(&event_key.as_bytes()[1..33], player.as_slice());
        assert_eq!(event_key.as_bytes()[33], PlayerEvent::Won as u8);
        assert_eq!(&event_key.as_bytes()[34..42], &version.to_be_bytes()[..]);
        assert_eq!(&event_key.as_bytes()[42..74], game.as_slice());

        let status_key = GameStatusKey::new(GameStatus::Playing, &game);
        assert_eq!(status_key.as_bytes()[0], GAME_STATUS_DISCRIMINATOR);
        assert_eq!(status_key.as_bytes()[1], GameStatus::Playing as u8);
        assert_eq!(&status_key.as_bytes()[2..34], game.as_slice());

        // Invalid lengths.
        assert_eq!(PlayerEventKey::parse(&[0u8; 73]), None);
        assert_eq!(PlayerEventKey::parse(&[0u8; 75]), None);
        assert_eq!(GameStatusKey::parse(&[0u8; 33]), None);
        assert_eq!(GameStatusKey::parse(&[0u8; 35]), None);

        // Invalid discriminator byte.
        let mut event_key_bad_disc = event_key.as_bytes().to_vec();
        event_key_bad_disc[0] = 0x00;
        assert_eq!(PlayerEventKey::parse(&event_key_bad_disc), None);
        let mut status_key_bad_disc = status_key.as_bytes().to_vec();
        status_key_bad_disc[0] = 0x01;
        assert_eq!(GameStatusKey::parse(&status_key_bad_disc), None);

        // Invalid event byte.
        let mut event_key_bad_event = event_key.as_bytes().to_vec();
        event_key_bad_event[33] = 0;
        assert_eq!(PlayerEventKey::parse(&event_key_bad_event), None);
        event_key_bad_event[33] = 6;
        assert_eq!(PlayerEventKey::parse(&event_key_bad_event), None);

        // Invalid status byte.
        let mut status_key_bad_status = status_key.as_bytes().to_vec();
        status_key_bad_status[1] = 3;
        assert_eq!(GameStatusKey::parse(&status_key_bad_status), None);
    }

    #[test]
    fn test_case_9_canonical_filtered_scans() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());

        let mut manager: CanonicalChainManager<u64> = CanonicalChainManager::default();
        manager.append(1u64);
        manager.append(2u64);
        // Rollback 2, append 3 -> versions 1 and 3 are canonical, version 2 is orphaned.
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
        // Player-event-log keys.
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
            PlayerEventKey::new(&player, PlayerEvent::Created, 3, &game3).as_bytes(),
            &[],
        );

        // Status keys in Open.
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

        // Player-event scan: all canonical events.
        let events = scan_player_events(&store, &snapshot, &player, PlayerEvent::Created, None, 10);
        assert_eq!(events, vec![(1, *game1), (3, *game3)]);

        // Player-event cursor pagination:
        // First page.
        let p1 = scan_player_events(&store, &snapshot, &player, PlayerEvent::Created, None, 1);
        assert_eq!(p1, vec![(1, *game1)]);

        // Middle page after (1, game1) -> skips orphaned version 2 and returns (3, game3).
        let p2 = scan_player_events(
            &store,
            &snapshot,
            &player,
            PlayerEvent::Created,
            Some((1, *game1)),
            1,
        );
        assert_eq!(p2, vec![(3, *game3)]);

        // Exhausted after (3, game3).
        let p3 = scan_player_events(
            &store,
            &snapshot,
            &player,
            PlayerEvent::Created,
            Some((3, *game3)),
            1,
        );
        assert!(p3.is_empty());

        // Status scan: all open canonical games in game-id ascending order.
        let open_games = scan_games_by_status(&store, &snapshot, GameStatus::Open, None, 10);
        assert_eq!(open_games, vec![*game1, *game3]);

        // Status cursor pagination:
        let s1 = scan_games_by_status(&store, &snapshot, GameStatus::Open, None, 1);
        assert_eq!(s1, vec![*game1]);

        let s2 = scan_games_by_status(&store, &snapshot, GameStatus::Open, Some(game1), 1);
        assert_eq!(s2, vec![*game3]);

        let s3 = scan_games_by_status(&store, &snapshot, GameStatus::Open, Some(game3), 1);
        assert!(s3.is_empty());
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

        // Seed store with both games in Open.
        let mut wb0 = store.write_batch();
        indexer.index_diff(&game1, None, Some(&open_bytes1), 1, &mut wb0);
        indexer.index_diff(&game2, None, Some(&open_bytes2), 1, &mut wb0);
        store.commit(wb0);

        let get = |status: GameStatus, game: &ResourceId| {
            store.get(StateSpace::Index, GameStatusKey::new(status, game).as_bytes())
        };
        assert_eq!(get(GameStatus::Open, &game1), Some(1u64.to_be_bytes().to_vec()));
        assert_eq!(get(GameStatus::Open, &game2), Some(1u64.to_be_bytes().to_vec()));

        // Transition only game 1: Open -> Playing at version 2.
        let mut wb = store.write_batch();
        indexer.index_diff(&game1, Some(&open_bytes1), Some(&playing_bytes1), 2, &mut wb);
        store.commit(wb);

        // Game 1's Open entry is gone, Playing entry is present.
        assert_eq!(get(GameStatus::Open, &game1), None);
        assert_eq!(get(GameStatus::Playing, &game1), Some(2u64.to_be_bytes().to_vec()));

        // Game 2's Open entry is untouched.
        assert_eq!(get(GameStatus::Open, &game2), Some(1u64.to_be_bytes().to_vec()));
    }

    #[test]
    fn test_status_maintenance_spares_event_log_keys() {
        let dir = TempDir::new().unwrap();
        let store: RocksDbStore = RocksDbStore::open(dir.path());
        let indexer = TicTacToeIndexer;

        // Player ID begins with 0x01 (same as the Playing status byte; the player-event-log
        // discriminator is 0x01, the game-status one 0x02).
        let player = rid(0x01);
        let joiner = rid(0x20);
        let game_id = rid(0x01);

        let open_bytes = make_game_bytes(State::Open, &player, None);
        let playing_bytes = make_game_bytes(State::Playing, &player, Some(&joiner));

        // Seed an event-log key for player and initial status entry.
        let mut wb0 = store.write_batch();
        let created_key = PlayerEventKey::new(&player, PlayerEvent::Created, 1, &game_id);
        wb0.put(StateSpace::Index, created_key.as_bytes(), &[]);
        wb0.put(
            StateSpace::Index,
            GameStatusKey::new(GameStatus::Open, &game_id).as_bytes(),
            &1u64.to_be_bytes(),
        );
        store.commit(wb0);

        // Transition: Open -> Playing.
        let mut wb = store.write_batch();
        indexer.index_diff(&game_id, Some(&open_bytes), Some(&playing_bytes), 2, &mut wb);
        store.commit(wb);

        // The event-log key must survive.
        assert_eq!(store.get(StateSpace::Index, created_key.as_bytes()), Some(vec![]));
        // Open is gone, Playing is present.
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
