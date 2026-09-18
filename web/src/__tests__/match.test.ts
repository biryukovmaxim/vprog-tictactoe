//! Match-panel pins: to-move parity, my-turn seat mapping, outcome copy
//! (both seat wins + draw split), and the activity-chip walker over DA polls.

import { describe, expect, it } from 'vitest';
import type { DaGame, GameStatus } from '../da';
import { advanceActivity, isMyTurn, outcomeLine, rowStuck, STUCK_AFTER_MS, toMoveMark, type ActivityWitness } from '../match';
import type { ActivityRow } from '../composition';

const ME = 'aa'.repeat(32);
const OP = 'bb'.repeat(32);

function game(partial: Partial<DaGame> = {}): DaGame {
  return {
    id: '11'.repeat(32),
    state: 1,
    state_name: 'Playing',
    stake: 50_000_000,
    pot: 100_000_000,
    rounds_total: 3,
    creator_mark: 1,
    round_wins: [0, 0],
    draws: 0,
    players: [ME, OP],
    board: Array<number>(9).fill(0),
    last_move_at: 0,
    ...partial,
  };
}

describe('toMoveMark (X when board parity even)', () => {
  it('X moves on an empty board', () => {
    expect(toMoveMark(Array<number>(9).fill(0))).toBe(1);
  });

  it('O moves after one mark', () => {
    expect(toMoveMark([1, 0, 0, 0, 0, 0, 0, 0, 0])).toBe(2);
  });

  it('X moves again after two marks', () => {
    expect(toMoveMark([1, 2, 0, 0, 0, 0, 0, 0, 0])).toBe(1);
  });

  it('O moves after five marks', () => {
    const board = [1, 2, 1, 2, 1, 0, 0, 0, 0];
    expect(toMoveMark(board)).toBe(2);
  });
});

describe('isMyTurn', () => {
  it('round 0, empty board, X creator: seat 0 moves', () => {
    expect(isMyTurn(game(), ME)).toBe(true);
    expect(isMyTurn(game(), OP)).toBe(false);
  });

  it('round 0, one mark: seat 1 moves', () => {
    const g = game({ board: [1, 0, 0, 0, 0, 0, 0, 0, 0] });
    expect(isMyTurn(g, ME)).toBe(false);
    expect(isMyTurn(g, OP)).toBe(true);
  });

  it('round 1 (marks swap seats): seat 1 opens on the fresh board', () => {
    const g = game({ round_wins: [1, 0] });
    expect(isMyTurn(g, ME)).toBe(false);
    expect(isMyTurn(g, OP)).toBe(true);
  });

  it('round 1, one mark: seat 0 moves', () => {
    const g = game({ round_wins: [1, 0], board: [0, 0, 1, 0, 0, 0, 0, 0, 0] });
    expect(isMyTurn(g, ME)).toBe(true);
    expect(isMyTurn(g, OP)).toBe(false);
  });

  it('false while Open (no joiner yet) and when finished', () => {
    expect(isMyTurn(game({ state: 0, state_name: 'Open', players: [ME, null] }), ME)).toBe(false);
    expect(isMyTurn(game({ state: 2, state_name: 'First' }), ME)).toBe(false);
    expect(isMyTurn(game({ state: 4, state_name: 'Draw' }), ME)).toBe(false);
  });

  it('false for an observer outside the players', () => {
    expect(isMyTurn(game(), 'cc'.repeat(32))).toBe(false);
  });

  // Regression pin: an O creator flips the seat-to-mark mapping — the joiner
  // holds X and opens; the creator takes X back in odd rounds.
  it('O creator, round 0: the joiner (X) moves, not the creator', () => {
    const g = game({ creator_mark: 2 });
    expect(isMyTurn(g, ME)).toBe(false);
    expect(isMyTurn(g, OP)).toBe(true);
  });

  it('O creator, round 0, one mark: the creator moves', () => {
    const g = game({ creator_mark: 2, board: [0, 0, 0, 0, 1, 0, 0, 0, 0] });
    expect(isMyTurn(g, ME)).toBe(true);
    expect(isMyTurn(g, OP)).toBe(false);
  });

  it('O creator, round 1: the creator (X in odd rounds) opens', () => {
    const g = game({ creator_mark: 2, round_wins: [1, 0] });
    expect(isMyTurn(g, ME)).toBe(true);
    expect(isMyTurn(g, OP)).toBe(false);
  });

  it('O creator, round 1, one mark: the joiner moves', () => {
    const g = game({ creator_mark: 2, round_wins: [1, 0], board: [1, 0, 0, 0, 0, 0, 0, 0, 0] });
    expect(isMyTurn(g, ME)).toBe(false);
    expect(isMyTurn(g, OP)).toBe(true);
  });
});

describe('outcomeLine', () => {
  it('seat 0 win: you when seat 0 is mine, opponent when seat 1 is mine', () => {
    expect(outcomeLine(2, [ME, OP], ME, 50_000_000n)).toBe('You win the pot');
    expect(outcomeLine(2, [ME, OP], OP, 50_000_000n)).toBe('Opponent wins the pot');
  });

  it('seat 1 win: mirrored', () => {
    expect(outcomeLine(3, [ME, OP], OP, 50_000_000n)).toBe('You win the pot');
    expect(outcomeLine(3, [ME, OP], ME, 50_000_000n)).toBe('Opponent wins the pot');
  });

  it('observer sees the plain seat copy', () => {
    expect(outcomeLine(2, [ME, OP], 'cc'.repeat(32), 50_000_000n)).toBe('Seat 0 wins the pot');
    expect(outcomeLine(3, [ME, OP], 'cc'.repeat(32), 50_000_000n)).toBe('Seat 1 wins the pot');
  });

  it('draw splits the stakes back, one stake each', () => {
    expect(outcomeLine(4, [ME, OP], ME, 50_000_000n)).toBe('Draw — stakes split back (0.50 KAS each)');
  });

  it('null unless the state is finished (>= 2)', () => {
    expect(outcomeLine(0, [ME, null], ME, 50_000_000n)).toBeNull();
    expect(outcomeLine(1, [ME, OP], ME, 50_000_000n)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Activity-chip walker.

function row(status: ActivityRow['status'], witness?: ActivityWitness, settledTxid?: string | null, at = 0): ActivityRow {
  return { id: 1, label: 'turn 4', txid: 'ff'.repeat(32), status, at, ...(witness ? { witness } : {}), ...(settledTxid !== undefined ? { settledTxid } : {}) };
}

function view(
  games: Partial<Record<GameStatus, DaGame[]>> = {},
  settledTxid: string | null = null,
  myBalance: bigint | null = null,
  myL1: bigint | null = null,
) {
  return {
    games: { open: [], playing: [], finished: [], ...games } as Record<GameStatus, DaGame[]>,
    settledTxid,
    myUserId: ME,
    myBalance,
    myL1,
  };
}

describe('advanceActivity (trust chips over DA polls)', () => {
  it('pending stays pending while the witnessed board is unchanged (same rows reference)', () => {
    const g = game({ board: [0, 0, 1, 0, 0, 0, 0, 0, 0] });
    const rows = [row('pending', { kind: 'board', gameId: g.id, board: g.board })];
    const out = advanceActivity(rows, view({ playing: [g] }));
    expect(out).toBe(rows);
    expect(out[0]!.status).toBe('pending');
  });

  it('pending turn flips to on L2 once the game body moved (board differs)', () => {
    const g = game({ board: [0, 0, 1, 0, 0, 0, 0, 0, 0] });
    const rows = [row('pending', { kind: 'board', gameId: g.id, board: [0, 0, 0, 0, 0, 0, 0, 0, 0] })];
    const out = advanceActivity(rows, view({ playing: [g] }, 'st-1'));
    expect(out[0]!.status).toBe('on L2');
    expect(out[0]!.settledTxid).toBe('st-1');
  });

  it('board witness looks across all status lists and waits when the game is absent', () => {
    const g = game({ state: 2, state_name: 'First', board: [1, 2, 1, 2, 1, 0, 0, 0, 0] });
    const witness: ActivityWitness = { kind: 'board', gameId: g.id, board: Array<number>(9).fill(0) };
    expect(advanceActivity([row('pending', witness)], view({ finished: [g] }))[0]!.status).toBe('on L2');
    expect(advanceActivity([row('pending', witness)], view())[0]!.status).toBe('pending');
  });

  it('my-games witness flips when my game count grew past the captured number', () => {
    const mine = game({ players: [ME, OP] });
    const theirs = game({ id: '22'.repeat(32), players: ['dd'.repeat(32), 'ee'.repeat(32)] });
    const rows = [row('pending', { kind: 'myGames', before: 0 })];
    // Others' games do not count; only mine flip the witness.
    expect(advanceActivity(rows, view({ playing: [theirs] }))).toBe(rows);
    const out = advanceActivity(rows, view({ playing: [mine, theirs] }, 'st-2'));
    expect(out[0]!.status).toBe('on L2');
  });

  it('on L2 flips to settled once the settlement txid changed after the ack', () => {
    const rows = [row('on L2', undefined, 'st-1')];
    expect(advanceActivity(rows, view({}, 'st-1'))[0]!.status).toBe('on L2');
    expect(advanceActivity(rows, view({}, 'st-9'))[0]!.status).toBe('settled');
    // A first settlement appearing after the ack also counts as a change.
    expect(advanceActivity([row('on L2', undefined, null)], view({}, 'st-1'))[0]!.status).toBe('settled');
  });

  it('balance witness (transfers, withdrawals) flips once my L2 balance moved', () => {
    const rows = [row('pending', { kind: 'balance', before: 100n })];
    // Same balance (and unknown balance) keep the row pending.
    expect(advanceActivity(rows, view({}, null, 100n))).toBe(rows);
    expect(advanceActivity(rows, view())).toBe(rows);
    const out = advanceActivity(rows, view({}, 'st-1', 50n));
    expect(out[0]!.status).toBe('on L2');
    expect(out[0]!.settledTxid).toBe('st-1');
  });

  it('l1 witness (exit claims) flips once my L1 UTXO sum rose past the captured one', () => {
    const rows = [row('pending', { kind: 'l1', before: 100n })];
    // A dip (a carrier's change spent) or an unread poll is not a payout ack.
    expect(advanceActivity(rows, view({}, null, null, 50n))).toBe(rows);
    expect(advanceActivity(rows, view())).toBe(rows);
    const out = advanceActivity(rows, view({}, 'st-1', null, 150n));
    expect(out[0]!.status).toBe('on L2');
    expect(out[0]!.settledTxid).toBe('st-1');
  });

  it('settled rows and witness-less rows stay as they are', () => {
    const rows = [row('settled', undefined, 'st-1'), row('pending')];
    expect(advanceActivity(rows, view({}, 'st-9'))).toBe(rows);
  });
});

describe('rowStuck (pending past the healthy window)', () => {
  it('a fresh pending row is not stuck', () => {
    expect(rowStuck(row('pending', undefined, undefined, 1_000), 1_000 + 10_000)).toBe(false);
  });

  it('a pending row past STUCK_AFTER_MS is stuck', () => {
    expect(rowStuck(row('pending', undefined, undefined, 1_000), 1_000 + STUCK_AFTER_MS + 1)).toBe(true);
  });

  it('acknowledged rows never flag', () => {
    expect(rowStuck(row('on L2', undefined, 'st-1', 1_000), 1_000 + STUCK_AFTER_MS * 10)).toBe(false);
  });
});
