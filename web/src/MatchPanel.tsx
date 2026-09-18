//! Match column: header round/phase, to-move line, board, tally, stake/pot,
//! and the outcome banner. Clicks submit one single-Turn carrier each, over
//! the same wallet/pickUtxo/fee-pad pipeline the create/join entries use.
//! Pending turns render as ghost marks (any mover's, straight from the L1
//! queue), queued premoves as numbered ghosts that fire when they become
//! legal — so the board stays clickable ahead of your turn.

import { useState } from 'react';
import { kas, sharedClient, submitTurn } from './composition';
import { inFlightTurns, useLaneCarriers } from './carriers';
import type { Identity } from './KeyBar';
import { Board, type Ghost } from './Board';
import { isMyTurn, outcomeLine, seatOf, toMoveMark, type ActivityWitness } from './match';
import { useDa, useGame } from './state';

/// One queued premove for this game (App owns the queue and the dispatcher).
export interface QueuedTurnView {
  id: number;
  cell: number;
}

export function MatchPanel({
  identity,
  gameId,
  myTurnInFlight,
  queue,
  onEnqueue,
  onCancelQueued,
  onActivity,
  onNeedsFunding,
}: {
  identity: Identity;
  gameId: string | null;
  /// One of my turns for this game is submitted but not yet on L2; direct
  /// submits pause until it lands (clicks fall through to the queue).
  myTurnInFlight: boolean;
  /// My queued premoves for this game, oldest first.
  queue: QueuedTurnView[];
  onEnqueue: (gameId: string, cell: number) => void;
  onCancelQueued: (id: number) => void;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
  onNeedsFunding: (needed: bigint) => void;
}) {
  const da = useDa();
  const game = useGame(gameId);
  const laneCarriers = useLaneCarriers();
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (!game) {
    return (
      <div className="stack">
        <h3>match</h3>
        <p className="hint">select a game in my games</p>
      </div>
    );
  }

  const finished = game.state >= 2;
  const completed = game.round_wins[0] + game.round_wins[1] + game.draws;
  const round = finished ? Math.min(completed, game.rounds_total) : completed + 1;
  const seat = seatOf(game.players, identity.userIdHex);
  const myTurn = isMyTurn(game, identity.userIdHex);
  const mover = toMoveMark(game.board) === 1 ? 'X' : 'O';
  const outcome = outcomeLine(game.state, game.players, identity.userIdHex, BigInt(game.stake));

  // Ghosts: in-flight turns of any mover (from the L1 queue) at the current
  // to-move mark, then my queued premoves numbered in firing order.
  const inFlight = inFlightTurns(laneCarriers, game.id);
  // My mark in the current round: the creator plays `creator_mark` in even
  // rounds and the opposite in odd ones (guest `mark_for_seat`).
  const completedRounds = game.round_wins[0] + game.round_wins[1] + game.draws;
  const seat0Mark: 1 | 2 = completedRounds % 2 === 0 ? (game.creator_mark === 2 ? 2 : 1) : game.creator_mark === 1 ? 2 : 1;
  const myGhostMark: 1 | 2 = seat === 1 ? (seat0Mark === 1 ? 2 : 1) : seat0Mark;
  const ghosts: Ghost[] = [
    ...inFlight.map((t) => ({ cell: t.cell, mark: toMoveMark(game.board) as 1 | 2 })),
    ...queue.map((q, i) => ({ cell: q.cell, mark: myGhostMark, order: i + 1 })),
  ];

  const play = async (cell: number) => {
    if (busy || game.state !== 1 || game.board[cell] !== 0) return;
    const lane = da.state?.lane_subnet;
    if (!lane) {
      setErr('waiting for DA state');
      return;
    }
    // Direct submit only from a clean slate; anything else is a premove.
    if (myTurn && !myTurnInFlight && queue.length === 0 && inFlight.length === 0) {
      setErr(null);
      setBusy(true);
      try {
        await submitTurn({ identity, client: await sharedClient(), lane, game, cell, onActivity, onNeedsFunding });
      } catch (e) {
        setErr(String(e));
      } finally {
        setBusy(false);
      }
    } else {
      setErr(null);
      onEnqueue(game.id, cell);
    }
  };

  const oppInFlight = inFlight.some((t) => t.userId !== identity.userIdHex);

  return (
    <div className="stack">
      <h3>match</h3>
      <div className="match-head">
        Round {round}/{game.rounds_total} · {game.state_name}
      </div>
      {game.state === 1 ? (
        <div>
          to move: {mover}
          {seat < 0 ? '' : myTurn ? ' (you)' : ' (opponent)'}
        </div>
      ) : (
        game.state === 0 && <div className="hint">waiting for a joiner</div>
      )}
      {myTurnInFlight && <div className="hint">your move is on L1 — the board unlocks when it lands on L2…</div>}
      {!myTurnInFlight && oppInFlight && <div className="hint">opponent's move is on L1…</div>}
      <Board board={game.board} enabled={game.state === 1} ghosts={ghosts} onCell={play} />
      {queue.length > 0 && (
        <div className="hint">
          queued:{' '}
          {queue.map((q) => (
            <button key={q.id} className="qchip" onClick={() => onCancelQueued(q.id)} title="cancel this premove">
              cell {q.cell} ✕
            </button>
          ))}
        </div>
      )}
      <div>
        wins {game.round_wins[0]}–{game.round_wins[1]} · draws {game.draws}
      </div>
      <div className="hint">
        stake {kas(game.stake)} · pot {kas(game.pot)}
      </div>
      {finished && outcome && <div className="outcome">{outcome}</div>}
      {err && <div className="err">{err}</div>}
    </div>
  );
}
