//! Pure tic-tac-toe rules: board queries, mark/seat derivation, and match outcome including the
//! early-clinch stop.
//!
//! Everything here is total and side-effect-free over plain values (no views, no resources), so
//! the game actions stay thin compositions and the rules themselves carry the tests.

use core::cmp::Ordering;

use crate::program::resources::game::{Cell, State};

/// The 8 winning lines, as board indexes.
const LINES: [[usize; 3]; 8] =
    [[0, 1, 2], [3, 4, 5], [6, 7, 8], [0, 3, 6], [1, 4, 7], [2, 5, 8], [0, 4, 8], [2, 4, 6]];

/// Number of marks on the board (the ply-in-round).
pub fn plies(board: &[Cell; 9]) -> u8 {
    board.iter().filter(|&&c| c != Cell::Empty).count() as u8
}

/// The mark holding a completed line, or `None` if no line is complete.
pub fn winner(board: &[Cell; 9]) -> Option<Cell> {
    LINES.iter().find_map(|&[a, b, c]| {
        (board[a] != Cell::Empty && board[a] == board[b] && board[b] == board[c])
            .then_some(board[a])
    })
}

/// Whether the board has no empty cell (a round is a draw when this holds and `winner` is
/// `None`).
pub fn board_full(board: &[Cell; 9]) -> bool {
    plies(board) == 9
}

/// The mark `seat` plays in `round` (0-based): seat 0 (the creator) plays `creator_mark` in
/// even rounds and the other mark in odd ones; seat 1 is mirrored.
pub fn mark_for_seat(creator_mark: Cell, round: u8, seat: usize) -> Cell {
    let creator_plays_own = round.is_multiple_of(2);
    match (seat, creator_plays_own) {
        (0, true) | (1, false) => creator_mark,
        (0, false) | (1, true) => other(creator_mark),
        _ => Cell::Empty,
    }
}

/// The seat whose turn it is: X opens every round, so the to-move mark is X on even ply and O
/// on odd, mapped back to a seat through `mark_for_seat`.
pub fn seat_to_move(creator_mark: Cell, round: u8, board: &[Cell; 9]) -> usize {
    let to_move = if plies(board).is_multiple_of(2) { Cell::X } else { Cell::O };
    if mark_for_seat(creator_mark, round, 0) == to_move { 0 } else { 1 }
}

/// The match result once `rounds_total - completed` rounds remain, or `None` while play
/// continues. Early clinch: a seat ahead by more than the remaining rounds cannot be caught,
/// so the leftover rounds are skipped and the match ends at once.
pub fn match_outcome(rounds_total: u8, round_wins: &[u8; 2], draws: u8) -> Option<State> {
    let w0 = round_wins[0] as u16;
    let w1 = round_wins[1] as u16;
    let completed = w0 + w1 + draws as u16;
    let remaining = (rounds_total as u16).saturating_sub(completed);
    if w0 > w1 + remaining {
        Some(State::First)
    } else if w1 > w0 + remaining {
        Some(State::Second)
    } else if remaining == 0 {
        Some(match w0.cmp(&w1) {
            Ordering::Greater => State::First,
            Ordering::Less => State::Second,
            Ordering::Equal => State::Draw,
        })
    } else {
        None
    }
}

/// The opposite mark; `Empty` maps to itself (unreachable for `creator_mark`, which
/// `write_game` restricts to X/O).
fn other(mark: Cell) -> Cell {
    match mark {
        Cell::X => Cell::O,
        Cell::O => Cell::X,
        Cell::Empty => Cell::Empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winner_finds_every_line_for_both_marks() {
        for &[i, j, k] in &LINES {
            for mark in [Cell::X, Cell::O] {
                let mut board = [Cell::Empty; 9];
                board[i] = mark;
                board[j] = mark;
                board[k] = mark;
                assert_eq!(winner(&board), Some(mark), "line {i},{j},{k} for {mark:?}");
            }
        }
    }

    #[test]
    fn winner_ignores_two_in_a_line() {
        let mut board = [Cell::Empty; 9];
        board[0] = Cell::X;
        board[1] = Cell::X;
        assert_eq!(winner(&board), None);
    }

    #[test]
    fn winner_ignores_mixed_line() {
        let mut board = [Cell::Empty; 9];
        board[0] = Cell::X;
        board[1] = Cell::X;
        board[2] = Cell::O;
        assert_eq!(winner(&board), None);
    }

    /// A full board with no line: the classic round-draw shape, `winner` `None` and full.
    #[test]
    fn full_board_without_a_line_is_a_draw() {
        let board = [
            Cell::X,
            Cell::O,
            Cell::X, //
            Cell::X,
            Cell::O,
            Cell::O, //
            Cell::O,
            Cell::X,
            Cell::X,
        ];
        assert_eq!(winner(&board), None);
        assert!(board_full(&board));
        assert_eq!(plies(&board), 9);
    }

    #[test]
    fn plies_counts_marks_only() {
        let mut board = [Cell::Empty; 9];
        assert_eq!(plies(&board), 0);
        board[4] = Cell::O;
        board[8] = Cell::X;
        assert_eq!(plies(&board), 2);
    }

    /// Seats always hold opposite marks within a round, and the creator's mark alternates by
    /// round parity.
    #[test]
    fn mark_for_seat_alternates_and_mirrors() {
        for creator_mark in [Cell::X, Cell::O] {
            for round in 0..4u8 {
                let s0 = mark_for_seat(creator_mark, round, 0);
                let s1 = mark_for_seat(creator_mark, round, 1);
                assert_ne!(s0, s1);
                assert_eq!(s0, if round % 2 == 0 { creator_mark } else { other(creator_mark) });
            }
        }
    }

    /// X opens every round regardless of who chose X; the seat mapping follows
    /// `creator_mark` and round parity.
    #[test]
    fn seat_to_move_tracks_ply_parity() {
        let mut board = [Cell::Empty; 9];
        assert_eq!(seat_to_move(Cell::X, 0, &board), 0);
        assert_eq!(seat_to_move(Cell::O, 0, &board), 1);

        board[4] = Cell::X;
        assert_eq!(seat_to_move(Cell::X, 0, &board), 1);
        assert_eq!(seat_to_move(Cell::O, 0, &board), 0);

        // Odd round with creator_mark = X: the joiner holds X and opens.
        let empty = [Cell::Empty; 9];
        assert_eq!(seat_to_move(Cell::X, 1, &empty), 1);
        assert_eq!(seat_to_move(Cell::O, 1, &empty), 0);
    }

    #[test]
    fn match_outcome_detects_early_clinch() {
        // 3-0 after 3 of 5 rounds: seat 1 can reach at most 2. The user's worked example.
        assert_eq!(match_outcome(5, &[3, 0], 0), Some(State::First));
        assert_eq!(match_outcome(5, &[0, 3], 0), Some(State::Second));
        // 2-1 after 3 of 5: nothing decided yet.
        assert_eq!(match_outcome(5, &[2, 1], 0), None);
        // 2-2 after 4 of 5: the last round decides.
        assert_eq!(match_outcome(5, &[2, 2], 0), None);
    }

    #[test]
    fn match_outcome_resolves_the_final_round() {
        assert_eq!(match_outcome(5, &[3, 2], 0), Some(State::First));
        assert_eq!(match_outcome(5, &[2, 3], 0), Some(State::Second));
        assert_eq!(match_outcome(5, &[2, 2], 1), Some(State::Draw));
        assert_eq!(match_outcome(1, &[1, 0], 0), Some(State::First));
        assert_eq!(match_outcome(1, &[0, 0], 1), Some(State::Draw));
        assert_eq!(match_outcome(3, &[2, 1], 0), Some(State::First));
        assert_eq!(match_outcome(3, &[0, 2], 1), Some(State::Second));
    }
}
