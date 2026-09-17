//! Pure match-panel helpers: to-move parity, my-turn seat mapping, outcome
//! copy, and the activity trust-chip walker over DA snapshots. Framework-free
//! by design (no React imports) — components live in the .tsx files.

import type { DaGame, GameStatus } from './da';
import type { ActivityRow } from './composition';

/// The mark to move: X opens every round, so X moves while the count of marks
/// on the board is even (guest `rules::seat_to_move` ply parity).
export function toMoveMark(board: number[]): 1 | 2 {
  let plies = 0;
  for (const c of board) if (c !== 0) plies++;
  return plies % 2 === 0 ? 1 : 2;
}

/// My seat in the game: 0 the creator, 1 the joiner, -1 an observer.
export function seatOf(players: [string, string | null], myUserId: string): 0 | 1 | -1 {
  if (players[0] === myUserId) return 0;
  if (players[1] === myUserId) return 1;
  return -1;
}

/// Whether the board accepts my click: the to-move mark comes from board
/// parity (X opens every round), and my own mark from the served
/// `creator_mark` — the creator plays it in even rounds and the opposite in
/// odd ones (guest `mark_for_seat`).
export function isMyTurn(game: DaGame, myUserId: string): boolean {
  if (game.state !== 1) return false;
  const seat = seatOf(game.players, myUserId);
  if (seat < 0) return false;
  const round = game.round_wins[0] + game.round_wins[1] + game.draws;
  const seat0Mark = round % 2 === 0 ? game.creator_mark : otherMark(game.creator_mark);
  const myMark = seat === 0 ? seat0Mark : otherMark(seat0Mark);
  return toMoveMark(game.board) === myMark;
}

/// The opposite mark code (1 X, 2 O).
function otherMark(mark: number): 1 | 2 {
  return mark === 1 ? 2 : 1;
}

const SOMPI = 100_000_000n;

function kas(amount: bigint): string {
  return `${(Number(amount) / Number(SOMPI)).toFixed(2)} KAS`;
}

/// Outcome banner copy for finished games (state >= 2); null while live.
/// Players see you/opponent; observers get the plain seat copy.
export function outcomeLine(
  state: number,
  players: [string, string | null],
  myUserId: string,
  stake: bigint,
): string | null {
  const seat = seatOf(players, myUserId);
  if (state === 2) {
    return seat === 0 ? 'You win the pot' : seat === 1 ? 'Opponent wins the pot' : 'Seat 0 wins the pot';
  }
  if (state === 3) {
    return seat === 1 ? 'You win the pot' : seat === 0 ? 'Opponent wins the pot' : 'Seat 1 wins the pot';
  }
  if (state === 4) return `Draw — stakes split back (${kas(stake)} each)`;
  return null;
}

// ---------------------------------------------------------------------------
// Activity trust chips: heuristic labels over the DA polls, no extra protocol.

/// What DA change acks one submitted action: the game's board for turns, my
/// game count for entries (create and join both add a game of mine), my L2
/// balance for transfers and withdrawals, my L1 UTXO sum rising for exit
/// claims (the payout lands on L1, the L2 side is untouched).
export type ActivityWitness =
  | { kind: 'board'; gameId: string; board: number[] }
  | { kind: 'myGames'; before: number }
  | { kind: 'balance'; before: bigint | null }
  | { kind: 'l1'; before: bigint | null };

/// The polled view the chip walk reads.
export interface ActivityView {
  games: Record<GameStatus, DaGame[]>;
  settledTxid: string | null;
  myUserId: string;
  myBalance: bigint | null;
  myL1: bigint | null;
}

/// Games across all statuses that seat my user.
export function myGamesCount(games: Record<GameStatus, DaGame[]>, myUserId: string): number {
  let n = 0;
  for (const list of [games.open, games.playing, games.finished])
    for (const g of list) if (g.players[0] === myUserId || g.players[1] === myUserId) n++;
  return n;
}

/// Walks the rows one poll: pending -> on L2 once the witnessed DA change
/// landed (game body moved / my game appeared), on L2 -> settled once the
/// settlement txid served by /api/state changed after that ack. Returns the
/// same array reference when no row advanced, so polling never re-renders.
export function advanceActivity(rows: ActivityRow[], view: ActivityView): ActivityRow[] {
  let changed = false;
  const out = rows.map((r) => {
    if (r.status === 'pending') {
      if (!acked(r.witness, view)) return r;
      changed = true;
      return { ...r, status: 'on L2' as const, settledTxid: view.settledTxid };
    }
    if (r.status === 'on L2' && r.settledTxid !== view.settledTxid) {
      changed = true;
      return { ...r, status: 'settled' as const };
    }
    return r;
  });
  return changed ? out : rows;
}

function acked(witness: ActivityWitness | undefined, view: ActivityView): boolean {
  if (!witness) return false;
  if (witness.kind === 'balance') return view.myBalance !== null && view.myBalance !== witness.before;
  if (witness.kind === 'l1') return view.myL1 !== null && witness.before !== null && view.myL1 > witness.before;
  if (witness.kind === 'myGames') return myGamesCount(view.games, view.myUserId) > witness.before;
  const g = [view.games.open, view.games.playing, view.games.finished]
    .flat()
    .find((game) => game.id === witness.gameId);
  return g !== undefined && !sameBoard(g.board, witness.board);
}

function sameBoard(a: number[], b: number[]): boolean {
  return a.length === b.length && a.every((c, i) => c === b[i]);
}
