//! Game resource (`derive_game_resource(creator_initial_lock_hash, games_started)`).
//!
//! Wire layout: kind byte + embedded match struct + inlined round board. A game carries no
//! lock, so unlike config/user there is no tag-driven tail: the payload is fully fixed-size,
//! created once at `GAME_WIRE_LEN`, and every later mutation is in-place.
//! ```text
//! [0]       kind          (GameKind::Game = 2; see `crate::program::resources::kind`)
//! [1]       state         (State; 0=Open 1=Playing 2=First 3=Second 4=Draw; finished ⟺ a
//!                         win/draw variant)
//! [2]       creator_mark  (Cell::X or Cell::O; the creator's mark in even rounds)
//! [3]       rounds_total  (u8)
//! [4..6]    round_wins    ([u8; 2])
//! [6]       draws         (u8)
//! [7..15]   stake         (u64 LE; the pot is always 2 x stake, not stored)
//! [15..23]  last_move_at  (u64 LE, ms of mergeset clock; 0 while Open)
//! [23..55]  players[0]    (creator ResourceId; never all-zero)
//! [55..87]  players[1]    (joiner ResourceId; all-zero while Open)
//! [87..99]  pending       (2x `cells [u8; 4] || head u8 || count u8`, seat-indexed)
//! [99..108] board         ([Cell; 9]; zeroized on round completion)
//! ```
//!
//! Derived, not stored: the current round is `round_wins[0] + round_wins[1] + draws`, the
//! ply-in-round is the count of marks on the board, and the to-move seat follows from ply
//! parity (X always opens a round) around `creator_mark`. The pending queues are per-seat
//! FIFOs of pre-committed cells (reorder tolerance): a turn for a future ply is appended to
//! the mover's own queue instead of rejected, and the queue head auto-applies when its turn
//! arrives; entries are owned structurally by the queue they sit in. Popped slots linger
//! (cursors only); the pair is zeroed at game end. Queue mutation belongs to the game
//! actions, not this view.

use vprogs_core_types::ResourceId;
use zerocopy::{
    FromZeros, Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned,
    little_endian::U64 as Le64,
};

use crate::program::resources::kind::GameKind;

/// Match lifecycle; the win/draw variants are exactly the finished states.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    FromZeros,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum State {
    /// Open for a joiner.
    Open = 0,
    /// Match in progress.
    Playing = 1,
    /// Finished: seat 0 (creator) takes the pot.
    First = 2,
    /// Finished: seat 1 (joiner) takes the pot.
    Second = 3,
    /// Finished: equal round wins; the stake returns to each seat exactly.
    Draw = 4,
}

/// A board cell; `creator_mark` reuses the same values minus `Empty` (rejected by
/// `write_game`), so the all-zero construction state parses.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    FromZeros,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum Cell {
    Empty = 0,
    X = 1,
    O = 2,
}

/// Per-seat pending-turn capacity.
pub const PENDING_CAP: usize = 4;

/// Total wire length of a game resource. Fixed: no lock tail, no variable body.
pub const GAME_WIRE_LEN: usize = core::mem::size_of::<GameRaw>();

/// One seat's ring of pre-committed cells in insert order: writes at
/// `(head + count) % PENDING_CAP`, pops at `head`. `count` gates everything (zero-fill is
/// inert), so popped slots need no purge; the cursors and body are zeroed at game end.
#[repr(C)]
#[derive(FromZeros, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct PendingQueue {
    pub cells: [u8; PENDING_CAP],
    pub head: u8,
    pub count: u8,
}

/// Whole-match fields; the embedded struct groups everything that lives for the game's
/// lifetime, zeroized per-unit (the queues at game end).
#[repr(C)]
#[derive(FromZeros, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct MatchRaw {
    pub state: State,
    pub creator_mark: Cell,
    pub rounds_total: u8,
    pub round_wins: [u8; 2],
    pub draws: u8,
    pub stake: Le64,
    pub last_move_at: Le64,
    pub players: [ResourceId; 2],
    pub pending: [PendingQueue; 2],
}

/// Zerocopy layout: kind discriminator + match struct + inlined round board.
#[repr(C)]
#[derive(FromZeros, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct GameRaw {
    pub kind: GameKind,
    pub m: MatchRaw,
    pub board: [Cell; 9],
}

/// Read-only view over a game resource. The enum bytes (kind, state, mark, board cells)
/// are discriminant-checked at `from_bytes` time, so accessors are infallible. Cross-field
/// invariants (mark parity, queue cursors, players) are owned by the writers: stored game
/// bytes only ever come from `write_game` and the game actions, so no reachable state can
/// carry them wrong.
pub struct GameView<'a>(&'a GameRaw);

impl<'a> GameView<'a> {
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, &'static str> {
        let raw = GameRaw::try_ref_from_bytes(bytes).map_err(|_| "game: invalid layout")?;
        Ok(Self(raw))
    }

    pub fn state(&self) -> State {
        self.0.m.state
    }

    /// The creator's mark in even rounds (the joiner's in odd).
    pub fn creator_mark(&self) -> Cell {
        self.0.m.creator_mark
    }

    pub fn rounds_total(&self) -> u8 {
        self.0.m.rounds_total
    }

    pub fn round_wins(&self) -> [u8; 2] {
        self.0.m.round_wins
    }

    pub fn draws(&self) -> u8 {
        self.0.m.draws
    }

    /// One seat's locked stake; the pot is `2 * stake`.
    pub fn stake(&self) -> u64 {
        self.0.m.stake.get()
    }

    /// When the last play was applied, in ms of the mergeset clock. 0 while Open.
    pub fn last_move_at(&self) -> u64 {
        self.0.m.last_move_at.get()
    }

    /// Seat 0's user id, set at creation.
    pub fn creator(&self) -> &'a ResourceId {
        &self.0.m.players[0]
    }

    /// Seat 1's user id, or `None` while the game is Open (all-zero id).
    pub fn joiner(&self) -> Option<&'a ResourceId> {
        let j = &self.0.m.players[1];
        (*j != ResourceId::default()).then_some(j)
    }

    /// The current round's board.
    pub fn board(&self) -> &'a [Cell; 9] {
        &self.0.board
    }

    /// Finished games are exactly the win/draw states.
    pub fn is_finished(&self) -> bool {
        matches!(self.0.m.state, State::First | State::Second | State::Draw)
    }
}

/// Mutable view for in-place updates. Setters do not re-validate; the writers own every
/// invariant. `stake`, `creator_mark`, `rounds_total` and seat 0 are create-time only by
/// construction.
pub struct GameViewMut<'a>(&'a mut GameRaw);

impl<'a> GameViewMut<'a> {
    pub fn from_bytes_mut(bytes: &'a mut [u8]) -> Result<Self, &'static str> {
        let raw = GameRaw::try_mut_from_bytes(bytes).map_err(|_| "game: invalid layout")?;
        Ok(Self(raw))
    }

    /// Mutable handle to the state.
    pub fn set_state(&mut self, v: State) {
        self.0.m.state = v;
    }

    /// Mutable handle to the per-seat round-win counters.
    pub fn round_wins_mut(&mut self) -> &mut [u8; 2] {
        &mut self.0.m.round_wins
    }

    /// Mutable handle to the draw counter.
    pub fn draws_mut(&mut self) -> &mut u8 {
        &mut self.0.m.draws
    }

    /// Sets the last-applied-play timestamp (ms of the mergeset clock).
    pub fn set_last_move_at(&mut self, v: u64) {
        self.0.m.last_move_at.set(v);
    }

    /// Fills the joiner seat.
    pub fn set_joiner(&mut self, id: &ResourceId) {
        self.0.m.players[1] = *id;
    }

    /// The current round's board; round completion is `board_mut().fill(Cell::Empty)`.
    pub fn board_mut(&mut self) -> &mut [Cell; 9] {
        &mut self.0.board
    }
}

/// Writes a fresh game wire buffer into `out`. `out` must be pre-sized to
/// [`GAME_WIRE_LEN`]. Every field not named here has a forced birth value: state Open,
/// counters zero, last_move_at 0, joiner unset, queues empty, board cleared.
pub fn write_game(
    out: &mut [u8],
    creator: &ResourceId,
    creator_mark: Cell,
    stake: u64,
    rounds_total: u8,
) -> Result<(), &'static str> {
    if out.len() != GAME_WIRE_LEN {
        return Err("game: write buffer wrong length");
    }
    if creator_mark == Cell::Empty {
        return Err("game: creator_mark must be X or O");
    }
    let raw = &mut GameRaw::new_zeroed();
    raw.kind = GameKind::Game;
    raw.m.state = State::Open;
    raw.m.creator_mark = creator_mark;
    raw.m.rounds_total = rounds_total;
    raw.m.stake = Le64::new(stake);
    raw.m.players[0] = *creator;
    out.copy_from_slice(raw.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn id(b: u8) -> ResourceId {
        ResourceId::from([b; 32])
    }

    /// Offset of the board within the wire buffer.
    const BOARD_OFF: usize = GAME_WIRE_LEN - 9;

    fn game_buf() -> alloc::vec::Vec<u8> {
        let mut buf = vec![0u8; GAME_WIRE_LEN];
        write_game(&mut buf, &id(0x11), Cell::X, 5_000, 3).unwrap();
        buf
    }

    #[test]
    fn round_trip_through_write_game() {
        let creator = id(0x11);
        let mut buf = vec![0u8; GAME_WIRE_LEN];
        write_game(&mut buf, &creator, Cell::O, 5_000, 3).unwrap();

        let view = GameView::from_bytes(&buf).unwrap();
        assert_eq!(view.state(), State::Open);
        assert_eq!(view.creator_mark(), Cell::O);
        assert_eq!(view.rounds_total(), 3);
        assert_eq!(view.round_wins(), [0, 0]);
        assert_eq!(view.draws(), 0);
        assert_eq!(view.stake(), 5_000);
        assert_eq!(view.last_move_at(), 0);
        assert_eq!(view.creator(), &creator);
        assert_eq!(view.joiner(), None);
        assert_eq!(view.board(), &[Cell::Empty; 9]);
        assert!(!view.is_finished());
    }

    #[test]
    fn write_game_rejects_empty_mark_and_wrong_length() {
        let creator = id(0x11);
        let mut buf = vec![0u8; GAME_WIRE_LEN];
        assert!(write_game(&mut buf, &creator, Cell::Empty, 5_000, 3).is_err());
        let mut short = vec![0u8; GAME_WIRE_LEN - 1];
        assert!(write_game(&mut short, &creator, Cell::X, 5_000, 3).is_err());
    }

    #[test]
    fn rejects_wrong_kind_byte() {
        // 1 is the user kind: not a valid game discriminant.
        let mut buf = game_buf();
        buf[0] = 1;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_unknown_state() {
        let mut buf = game_buf();
        buf[1] = 5;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(GameView::from_bytes(&[0u8; GAME_WIRE_LEN + 1]).is_err());
        assert!(GameView::from_bytes(&[0u8; GAME_WIRE_LEN - 1]).is_err());
    }

    #[test]
    fn rejects_bad_board_cell() {
        let mut buf = game_buf();
        buf[BOARD_OFF] = 3;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn mutable_view_updates_match_fields_and_board() {
        let joiner = id(0x22);
        let mut buf = game_buf();
        {
            let mut mv = GameViewMut::from_bytes_mut(&mut buf).unwrap();
            mv.set_state(State::Playing);
            mv.set_joiner(&joiner);
            mv.round_wins_mut()[1] = 2;
            *mv.draws_mut() = 1;
            mv.set_last_move_at(77_777);
            mv.board_mut()[0] = Cell::X;
            mv.board_mut()[4] = Cell::O;
        }

        let view = GameView::from_bytes(&buf).unwrap();
        assert_eq!(view.state(), State::Playing);
        assert_eq!(view.joiner(), Some(&joiner));
        assert_eq!(view.round_wins(), [0, 2]);
        assert_eq!(view.draws(), 1);
        assert_eq!(view.last_move_at(), 77_777);
        assert_eq!(view.board()[0], Cell::X);
        assert_eq!(view.board()[4], Cell::O);
        assert!(!view.is_finished());

        // A finished state flips the flag.
        GameViewMut::from_bytes_mut(&mut buf).unwrap().set_state(State::Draw);
        assert!(GameView::from_bytes(&buf).unwrap().is_finished());
    }

    #[test]
    fn board_clear_leaves_match_fields() {
        let joiner = id(0x22);
        let mut buf = game_buf();
        {
            let mut mv = GameViewMut::from_bytes_mut(&mut buf).unwrap();
            mv.set_state(State::Playing);
            mv.set_joiner(&joiner);
            mv.round_wins_mut()[0] = 1;
            mv.board_mut()[0] = Cell::X;
            mv.board_mut()[8] = Cell::O;
            mv.board_mut().fill(Cell::Empty);
        }

        let view = GameView::from_bytes(&buf).unwrap();
        assert_eq!(view.board(), &[Cell::Empty; 9]);
        assert_eq!(view.round_wins(), [1, 0]);
        assert_eq!(view.joiner(), Some(&joiner));
    }
}
