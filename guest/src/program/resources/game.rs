//! Game resource (`derive_game_resource(creator_initial_lock_hash, games_started)`).
//!
//! Wire layout: kind byte + embedded match struct + inlined round board. A game carries no
//! lock, so unlike config/user there is no tag-driven tail: the payload is fully fixed-size,
//! created once at `GAME_WIRE_LEN`, and every later mutation is in-place.
//! ```text
//! [0]       kind          (Kind::Game = 2; see `crate::program::resources::kind`)
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
//! The kind byte is checked by `from_bytes` before the kindless body is
//! zerocopy-parsed, so it carries no field of [`GameView`] itself.
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

use crate::program::resources::kind::{Kind, kind_of};

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
pub const GAME_WIRE_LEN: usize = 1 + core::mem::size_of::<GameView>();

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

impl PendingQueue {
    /// Appends `cell` in insert order, or returns `false` when the ring is full.
    pub fn push(&mut self, cell: u8) -> bool {
        if self.count as usize == PENDING_CAP {
            return false;
        }
        let tail = (self.head as usize + self.count as usize) % PENDING_CAP;
        self.cells[tail] = cell;
        self.count += 1;
        true
    }

    /// Pops the oldest entry; `None` when empty. The popped slot lingers (a cursor, not a
    /// purge) until its reuse or the game-end zeroing.
    pub fn pop(&mut self) -> Option<u8> {
        if self.count == 0 {
            return None;
        }
        let cell = self.cells[self.head as usize];
        self.head = (self.head + 1) % PENDING_CAP as u8;
        self.count -= 1;
        Some(cell)
    }

    /// The oldest entry without removing it; `None` when empty.
    pub fn peek(&self) -> Option<u8> {
        (self.count > 0).then(|| self.cells[self.head as usize])
    }

    /// Whether the queue holds no entries.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Whole-match fields; the embedded struct groups everything that lives for the game's
/// lifetime, zeroized per-unit (the queues at game end).
#[repr(C)]
#[derive(FromZeros, IntoBytes, Immutable, KnownLayout, Unaligned)]
struct MatchRaw {
    state: State,
    creator_mark: Cell,
    rounds_total: u8,
    round_wins: [u8; 2],
    draws: u8,
    stake: Le64,
    last_move_at: Le64,
    players: [ResourceId; 2],
    pending: [PendingQueue; 2],
}

/// Zerocopy layout over the kindless body: match struct + inlined round board.
/// Fields are private; a handle is obtainable only through the validating
/// `from_bytes` / `from_bytes_mut`.
///
/// The enum bytes (state, mark, board cells) are discriminant-checked at parse time, so
/// accessors are infallible. Cross-field invariants (mark parity, queue cursors, players)
/// are owned by the writers: stored game bytes only ever come from `write_game` and the
/// game actions, so no reachable state can carry them wrong.
#[repr(C)]
#[derive(FromZeros, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct GameView {
    m: MatchRaw,
    board: [Cell; 9],
}

impl GameView {
    /// Validates `bytes` (kind byte + body) and returns the read view over them.
    pub fn from_bytes(bytes: &[u8]) -> Result<&Self, &'static str> {
        if kind_of(bytes) != Some(Kind::Game) {
            return Err("game: wrong kind");
        }
        Self::try_ref_from_bytes(&bytes[1..]).map_err(|_| "game: invalid layout")
    }

    /// Mutable counterpart of [`Self::from_bytes`] for in-place updates. Setters do not
    /// re-validate; the writers own every invariant. `stake`, `creator_mark`,
    /// `rounds_total` and seat 0 are create-time only by construction.
    pub fn from_bytes_mut(bytes: &mut [u8]) -> Result<&mut Self, &'static str> {
        if kind_of(bytes) != Some(Kind::Game) {
            return Err("game: wrong kind");
        }
        Self::try_mut_from_bytes(&mut bytes[1..]).map_err(|_| "game: invalid layout")
    }

    pub fn state(&self) -> State {
        self.m.state
    }

    /// The creator's mark in even rounds (the joiner's in odd).
    pub fn creator_mark(&self) -> Cell {
        self.m.creator_mark
    }

    /// Match length in rounds, fixed at creation.
    pub fn rounds_total(&self) -> u8 {
        self.m.rounds_total
    }

    /// Per-seat round-win counters, seat-indexed.
    pub fn round_wins(&self) -> [u8; 2] {
        self.m.round_wins
    }

    /// Rounds ended without a winner.
    pub fn draws(&self) -> u8 {
        self.m.draws
    }

    /// One seat's locked stake; the pot is `2 * stake`.
    pub fn stake(&self) -> u64 {
        self.m.stake.get()
    }

    /// When the last play was applied, in ms of the mergeset clock. 0 while Open.
    pub fn last_move_at(&self) -> u64 {
        self.m.last_move_at.get()
    }

    /// Seat 0's user id, set at creation.
    pub fn creator(&self) -> &ResourceId {
        &self.m.players[0]
    }

    /// Seat 1's user id, or `None` while the game is Open (all-zero id).
    pub fn joiner(&self) -> Option<&ResourceId> {
        let j = &self.m.players[1];
        (*j != ResourceId::default()).then_some(j)
    }

    /// The current round's board.
    pub fn board(&self) -> &[Cell; 9] {
        &self.board
    }

    /// Finished games are exactly the win/draw states.
    pub fn is_finished(&self) -> bool {
        matches!(self.m.state, State::First | State::Second | State::Draw)
    }

    /// Mutable handle to the state.
    pub fn set_state(&mut self, v: State) {
        self.m.state = v;
    }

    /// Mutable handle to the per-seat round-win counters.
    pub fn round_wins_mut(&mut self) -> &mut [u8; 2] {
        &mut self.m.round_wins
    }

    /// Mutable handle to the draw counter.
    pub fn draws_mut(&mut self) -> &mut u8 {
        &mut self.m.draws
    }

    /// Sets the last-applied-play timestamp (ms of the mergeset clock).
    pub fn set_last_move_at(&mut self, v: u64) {
        self.m.last_move_at.set(v);
    }

    /// Fills the joiner seat.
    pub fn set_joiner(&mut self, id: &ResourceId) {
        self.m.players[1] = *id;
    }

    /// The current round's board; round completion is `board_mut().fill(Cell::Empty)`.
    pub fn board_mut(&mut self) -> &mut [Cell; 9] {
        &mut self.board
    }

    /// One seat's pending-queue ring; when to push or pop belongs to the game actions.
    pub fn pending_mut(&mut self, seat: usize) -> &mut PendingQueue {
        &mut self.m.pending[seat]
    }

    /// Zeroes both queues: match end only, since entries persist across rounds by design.
    pub fn clear_pending(&mut self) {
        for q in &mut self.m.pending {
            *q = PendingQueue::new_zeroed();
        }
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
    let mut raw = GameView::new_zeroed();
    raw.m.state = State::Open;
    raw.m.creator_mark = creator_mark;
    raw.m.rounds_total = rounds_total;
    raw.m.stake = Le64::new(stake);
    raw.m.players[0] = *creator;
    out[0] = Kind::Game as u8;
    out[1..].copy_from_slice(raw.as_bytes());
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
        // 1 is the user kind: not the game discriminator.
        let mut buf = game_buf();
        buf[0] = Kind::User as u8;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    /// Kind 0 is the config kind; it must not parse as a game even though the old
    /// `Unset` construction value sat there.
    #[test]
    fn rejects_config_kind_byte() {
        let mut buf = game_buf();
        buf[0] = Kind::Config as u8;
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
            let mv = GameView::from_bytes_mut(&mut buf).unwrap();
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
        GameView::from_bytes_mut(&mut buf).unwrap().set_state(State::Draw);
        assert!(GameView::from_bytes(&buf).unwrap().is_finished());
    }

    #[test]
    fn board_clear_leaves_match_fields() {
        let joiner = id(0x22);
        let mut buf = game_buf();
        {
            let mv = GameView::from_bytes_mut(&mut buf).unwrap();
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

    // Pending-queue ring

    #[test]
    fn queue_pops_in_push_order() {
        let mut q = PendingQueue::new_zeroed();
        for cell in [1u8, 2, 3] {
            assert!(q.push(cell));
        }
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), Some(3));
        assert!(q.is_empty());
    }

    #[test]
    fn queue_rejects_push_past_capacity() {
        let mut q = PendingQueue::new_zeroed();
        for cell in 0u8..PENDING_CAP as u8 {
            assert!(q.push(cell));
        }
        assert!(!q.push(9));
    }

    /// Pop-then-push wraps the tail back over drained slots without disturbing order.
    #[test]
    fn queue_wraps_after_pops() {
        let mut q = PendingQueue::new_zeroed();
        q.push(10);
        q.push(11);
        q.push(12);
        assert_eq!(q.pop(), Some(10));
        assert_eq!(q.pop(), Some(11));
        assert!(q.push(13));
        assert!(q.push(14));
        assert_eq!(q.pop(), Some(12));
        assert_eq!(q.pop(), Some(13));
        assert_eq!(q.pop(), Some(14));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn queue_pop_on_empty_is_none() {
        let mut q = PendingQueue::new_zeroed();
        assert_eq!(q.pop(), None);
    }

    /// Draining leaves stale bytes in `cells`; `count == 0` keeps them inert on re-push
    /// (every live slot is rewritten) and on pop (never reached).
    #[test]
    fn drained_slots_linger_inertly() {
        let mut q = PendingQueue::new_zeroed();
        q.push(7);
        assert_eq!(q.pop(), Some(7));
        assert_eq!(q.cells[0], 7, "slot lingers by design");
        assert!(q.is_empty());
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn view_pending_is_seat_indexed_and_clearable() {
        let mut buf = game_buf();
        {
            let mv = GameView::from_bytes_mut(&mut buf).unwrap();
            mv.pending_mut(0).push(3);
            mv.pending_mut(1).push(5);
            mv.pending_mut(1).push(6);
        }
        {
            let mv = GameView::from_bytes_mut(&mut buf).unwrap();
            assert_eq!(mv.pending_mut(0).pop(), Some(3));
            assert_eq!(mv.pending_mut(1).pop(), Some(5));
            assert_eq!(mv.pending_mut(1).pop(), Some(6));
        }
        {
            let mv = GameView::from_bytes_mut(&mut buf).unwrap();
            mv.pending_mut(0).push(1);
            mv.pending_mut(1).push(2);
            mv.clear_pending();
        }
        let mv = GameView::from_bytes_mut(&mut buf).unwrap();
        assert!(mv.pending_mut(0).is_empty());
        assert!(mv.pending_mut(1).is_empty());
    }
}
