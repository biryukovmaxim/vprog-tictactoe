//! Game resource (`derive_game_resource(creator_initial_lock_hash, games_started)`).
//!
//! Wire layout: kind byte + embedded match struct + inlined round board. A game carries no
//! lock, so unlike config/user there is no tag-driven tail: the payload is fully fixed-size,
//! created once at `GAME_WIRE_LEN`, and every later mutation is in-place.
//! ```text
//! [0]       kind          (KIND_GAME = 2; see `crate::program::kind`)
//! [1]       state         (u8; 0=Open 1=Playing 2=First 3=Second 4=Draw; finished ⟺ >= 2)
//! [2]       creator_mark  (u8; 1=X 2=O; the creator's mark in even rounds)
//! [3]       rounds_total  (u8)
//! [4..6]    round_wins    ([u8; 2])
//! [6]       draws         (u8)
//! [7..15]   stake         (u64 LE; the pot is always 2 x stake, not stored)
//! [15..23]  last_move_at  (u64 LE, ms of mergeset clock; 0 while Open)
//! [23..55]  players[0]    (creator ResourceId; never all-zero)
//! [55..87]  players[1]    (joiner ResourceId; all-zero while Open)
//! [87..99]  pending       (2x `cells [u8; 4] || head u8 || count u8`, seat-indexed)
//! [99..108] board         ([u8; 9]; 0=Empty 1=X 2=O; zeroized on round completion)
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
    FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned, little_endian::U64 as Le64,
};

use crate::program::kind::KIND_GAME;

/// Game is open for a joiner.
pub const STATE_OPEN: u8 = 0;
/// Match in progress.
pub const STATE_PLAYING: u8 = 1;
/// Finished: seat 0 (creator) takes the pot.
pub const STATE_FIRST: u8 = 2;
/// Finished: seat 1 (joiner) takes the pot.
pub const STATE_SECOND: u8 = 3;
/// Finished: equal round wins; the stake returns to each seat exactly.
pub const STATE_DRAW: u8 = 4;

/// Mark values; double as the board's non-empty cell values.
pub const MARK_X: u8 = 1;
pub const MARK_O: u8 = 2;

/// Per-seat pending-turn capacity.
pub const PENDING_CAP: usize = 4;

/// Total wire length of a game resource. Fixed: no lock tail, no variable body.
pub const GAME_WIRE_LEN: usize = core::mem::size_of::<GameRaw>();

/// One seat's ring of pre-committed cells in insert order: writes at
/// `(head + count) % PENDING_CAP`, pops at `head`. `count` gates everything (zero-fill is
/// inert), so popped slots need no purge; the cursors and body are zeroed at game end.
#[repr(C)]
#[derive(Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct PendingQueue {
    pub cells: [u8; PENDING_CAP],
    pub head: u8,
    pub count: u8,
}

/// Whole-match fields; the embedded struct groups everything that lives for the game's
/// lifetime, zeroized per-unit (the queues at game end).
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct MatchRaw {
    pub state: u8,
    pub creator_mark: u8,
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
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct GameRaw {
    pub kind: u8,
    pub m: MatchRaw,
    pub board: [u8; 9],
}

/// Read-only view over a game resource. Shape and invariants validated at `from_bytes`
/// time, so accessors are infallible.
pub struct GameView<'a>(&'a GameRaw);

impl<'a> GameView<'a> {
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.len() != GAME_WIRE_LEN {
            return Err("game: wrong length");
        }
        let raw = GameRaw::ref_from_bytes(bytes).map_err(|_| "game: invalid layout")?;
        validate(raw)?;
        Ok(Self(raw))
    }

    pub fn state(&self) -> u8 {
        self.0.m.state
    }

    /// The creator's mark in even rounds (the joiner's in odd); X or O.
    pub fn creator_mark(&self) -> u8 {
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

    /// Seat 0's user id. Set at creation; never the all-zero id.
    pub fn creator(&self) -> &'a ResourceId {
        &self.0.m.players[0]
    }

    /// Seat 1's user id, or `None` while the game is Open (all-zero id).
    pub fn joiner(&self) -> Option<&'a ResourceId> {
        let j = &self.0.m.players[1];
        (*j != ResourceId::default()).then_some(j)
    }

    /// The current round's board; 0=Empty 1=X 2=O.
    pub fn board(&self) -> &'a [u8; 9] {
        &self.0.board
    }

    /// Finished games are exactly the states at or above [`STATE_FIRST`].
    pub fn is_finished(&self) -> bool {
        self.0.m.state >= STATE_FIRST
    }
}

/// Mutable view for in-place updates. Setters do not re-validate; `from_bytes` checks run
/// again whenever the buffer is re-viewed, and the game actions write only reachable states.
/// `stake`, `creator_mark`, `rounds_total` and seat 0 are create-time only by construction.
pub struct GameViewMut<'a>(&'a mut GameRaw);

impl<'a> GameViewMut<'a> {
    pub fn from_bytes_mut(bytes: &'a mut [u8]) -> Result<Self, &'static str> {
        if bytes.len() != GAME_WIRE_LEN {
            return Err("game: wrong length");
        }
        let raw = GameRaw::mut_from_bytes(bytes).map_err(|_| "game: invalid layout")?;
        validate(raw)?;
        Ok(Self(raw))
    }

    /// Mutable handle to the state byte.
    pub fn set_state(&mut self, v: u8) {
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

    /// The current round's board; round completion is `board_mut().fill(0)`.
    pub fn board_mut(&mut self) -> &mut [u8; 9] {
        &mut self.0.board
    }
}

/// Shared `from_bytes`/`from_bytes_mut` validation: every byte-range and invariant a
/// reachable game state satisfies.
fn validate(raw: &GameRaw) -> Result<(), &'static str> {
    if raw.kind != KIND_GAME {
        return Err("game: wrong kind byte");
    }
    let m = &raw.m;
    if m.state > STATE_DRAW {
        return Err("game: unknown state");
    }
    if m.creator_mark != MARK_X && m.creator_mark != MARK_O {
        return Err("game: creator_mark must be X or O");
    }
    if m.players[0] == ResourceId::default() {
        return Err("game: creator seat unset");
    }
    for q in &m.pending {
        if q.head as usize >= PENDING_CAP || q.count as usize > PENDING_CAP {
            return Err("game: pending queue cursors out of range");
        }
        if q.cells.iter().any(|&c| c >= 9) {
            return Err("game: pending cell out of range");
        }
    }
    let mut xs = 0usize;
    let mut os = 0usize;
    for &cell in &raw.board {
        match cell {
            0 => {}
            MARK_X => xs += 1,
            MARK_O => os += 1,
            _ => return Err("game: board cell not empty/X/O"),
        }
    }
    // X opens every round, so the mark counts can never differ by more than X's single
    // move-in-hand.
    if os > xs || xs > os + 1 {
        return Err("game: impossible mark counts on board");
    }
    Ok(())
}

/// Writes a fresh game wire buffer into `out`. `out` must be pre-sized to
/// [`GAME_WIRE_LEN`]. Every field not named here has a forced birth value: state Open,
/// counters zero, last_move_at 0, joiner unset, queues empty, board cleared.
pub fn write_game(
    out: &mut [u8],
    creator: &ResourceId,
    creator_mark: u8,
    stake: u64,
    rounds_total: u8,
) -> Result<(), &'static str> {
    if out.len() != GAME_WIRE_LEN {
        return Err("game: write buffer wrong length");
    }
    if creator_mark != MARK_X && creator_mark != MARK_O {
        return Err("game: creator_mark must be X or O");
    }
    let raw = GameRaw::mut_from_bytes(out).map_err(|_| "game: invalid layout")?;
    raw.kind = KIND_GAME;
    raw.m.state = STATE_OPEN;
    raw.m.creator_mark = creator_mark;
    raw.m.rounds_total = rounds_total;
    raw.m.round_wins = [0; 2];
    raw.m.draws = 0;
    raw.m.stake = Le64::new(stake);
    raw.m.last_move_at = Le64::new(0);
    raw.m.players = [*creator, ResourceId::default()];
    let empty = PendingQueue { cells: [0; PENDING_CAP], head: 0, count: 0 };
    raw.m.pending = [empty, empty];
    raw.board = [0; 9];
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn id(b: u8) -> ResourceId {
        ResourceId::from([b; 32])
    }

    /// Offset of the pending-queue pair within the wire buffer.
    const PENDING_OFF: usize =
        core::mem::offset_of!(GameRaw, m) + core::mem::offset_of!(MatchRaw, pending);
    /// Offset of the board within the wire buffer.
    const BOARD_OFF: usize = GAME_WIRE_LEN - 9;

    fn game_buf() -> alloc::vec::Vec<u8> {
        let mut buf = vec![0u8; GAME_WIRE_LEN];
        write_game(&mut buf, &id(0x11), MARK_X, 5_000, 3).unwrap();
        buf
    }

    #[test]
    fn round_trip_through_write_game() {
        let creator = id(0x11);
        let mut buf = vec![0u8; GAME_WIRE_LEN];
        write_game(&mut buf, &creator, MARK_O, 5_000, 3).unwrap();

        let view = GameView::from_bytes(&buf).unwrap();
        assert_eq!(view.state(), STATE_OPEN);
        assert_eq!(view.creator_mark(), MARK_O);
        assert_eq!(view.rounds_total(), 3);
        assert_eq!(view.round_wins(), [0, 0]);
        assert_eq!(view.draws(), 0);
        assert_eq!(view.stake(), 5_000);
        assert_eq!(view.last_move_at(), 0);
        assert_eq!(view.creator(), &creator);
        assert_eq!(view.joiner(), None);
        assert_eq!(view.board(), &[0u8; 9]);
        assert!(!view.is_finished());
    }

    #[test]
    fn write_game_rejects_bad_mark_and_length() {
        let creator = id(0x11);
        let mut buf = vec![0u8; GAME_WIRE_LEN];
        assert!(write_game(&mut buf, &creator, 0, 5_000, 3).is_err());
        assert!(write_game(&mut buf, &creator, 3, 5_000, 3).is_err());
        let mut short = vec![0u8; GAME_WIRE_LEN - 1];
        assert!(write_game(&mut short, &creator, MARK_X, 5_000, 3).is_err());
    }

    #[test]
    fn rejects_wrong_kind_byte() {
        let mut buf = game_buf();
        buf[0] = crate::program::kind::KIND_USER;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_unknown_state() {
        let mut buf = game_buf();
        buf[1] = STATE_DRAW + 1;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_bad_creator_mark() {
        for bad in [0u8, MARK_O + 1] {
            let mut buf = game_buf();
            buf[2] = bad;
            assert!(GameView::from_bytes(&buf).is_err());
        }
    }

    #[test]
    fn rejects_zero_creator_seat() {
        let mut buf = game_buf();
        buf[23..55].fill(0);
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
    fn rejects_impossible_mark_counts() {
        // O leading: X opens every round.
        let mut buf = game_buf();
        buf[BOARD_OFF] = MARK_O;
        assert!(GameView::from_bytes(&buf).is_err());

        // X two ahead: moves strictly alternate within a round.
        let mut buf = game_buf();
        buf[BOARD_OFF] = MARK_X;
        buf[BOARD_OFF + 1] = MARK_X;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_queue_cell_out_of_range() {
        let mut buf = game_buf();
        buf[PENDING_OFF] = 9;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_queue_cursor_overflow() {
        // Seat 1's count byte (cells [u8; 4] || head || count).
        let mut buf = game_buf();
        buf[PENDING_OFF + 2 * core::mem::size_of::<PendingQueue>() + PENDING_CAP + 1] = 5;
        assert!(GameView::from_bytes(&buf).is_err());

        // Seat 0's head byte.
        let mut buf = game_buf();
        buf[PENDING_OFF + PENDING_CAP] = PENDING_CAP as u8;
        assert!(GameView::from_bytes(&buf).is_err());
    }

    #[test]
    fn mutable_view_updates_match_fields_and_board() {
        let joiner = id(0x22);
        let mut buf = game_buf();
        {
            let mut mv = GameViewMut::from_bytes_mut(&mut buf).unwrap();
            mv.set_state(STATE_PLAYING);
            mv.set_joiner(&joiner);
            mv.round_wins_mut()[1] = 2;
            *mv.draws_mut() = 1;
            mv.set_last_move_at(77_777);
            mv.board_mut()[0] = MARK_X;
            mv.board_mut()[4] = MARK_O;
        }

        let view = GameView::from_bytes(&buf).unwrap();
        assert_eq!(view.state(), STATE_PLAYING);
        assert_eq!(view.joiner(), Some(&joiner));
        assert_eq!(view.round_wins(), [0, 2]);
        assert_eq!(view.draws(), 1);
        assert_eq!(view.last_move_at(), 77_777);
        assert_eq!(view.board()[0], MARK_X);
        assert_eq!(view.board()[4], MARK_O);
        assert!(!view.is_finished());
    }

    #[test]
    fn board_clear_leaves_match_fields() {
        let joiner = id(0x22);
        let mut buf = game_buf();
        {
            let mut mv = GameViewMut::from_bytes_mut(&mut buf).unwrap();
            mv.set_state(STATE_PLAYING);
            mv.set_joiner(&joiner);
            mv.round_wins_mut()[0] = 1;
            mv.board_mut()[0] = MARK_X;
            mv.board_mut()[8] = MARK_O;
            mv.board_mut().fill(0);
        }

        let view = GameView::from_bytes(&buf).unwrap();
        assert_eq!(view.board(), &[0u8; 9]);
        assert_eq!(view.round_wins(), [1, 0]);
        assert_eq!(view.joiner(), Some(&joiner));
    }
}
