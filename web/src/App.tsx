//! One page: KeyBar, settlement banner, three columns (game lists, match,
//! actions + activity). Rows enter the activity log `pending` at submit and
//! the DA poll walks their trust chips via match.ts `advanceActivity`. Turns
//! clicked while not immediately playable land in the premove queue and are
//! dispatched by the effect below the moment they become legal.

import { useCallback, useEffect, useRef, useState } from 'react';
import { pickUtxo, sharedClient, submitTurn, useMyBalances, type ActivityRow } from './composition';
import { inFlightTurns, useLaneCarriers } from './carriers';
import { ActivityLog } from './ActivityLog';
import { ClaimExits } from './ClaimExits';
import { KeyBar, type Identity } from './KeyBar';
import { CreatePanel } from './CreatePanel';
import { LaneQueue } from './LaneQueue';
import { MatchPanel } from './MatchPanel';
import { MyGames } from './MyGames';
import { OpenGames } from './OpenGames';
import { SettlementBanner } from './SettlementBanner';
import { TransferForm } from './TransferForm';
import { WithdrawForm } from './WithdrawForm';
import { advanceActivity, myGamesCount, turnDispatchable, type ActivityWitness } from './match';
import { useDa } from './state';

/// Strictly monotonic activity-row id: `rows.length` duplicates once the
/// 50-row cap starts slicing (length saturates at 50 while rows churn).
let nextActivityId = 0;

/// One pre-authored turn waiting for its moment: submitted the instant it is
/// my move on an empty cell with nothing in flight (see the dispatch effect).
interface QueuedTurn {
  id: number;
  gameId: string;
  cell: number;
}
let nextQueueId = 0;

export default function App() {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [selectedGame, setSelectedGame] = useState<string | null>(null);
  const [activity, setActivity] = useState<ActivityRow[]>([]);
  const [queue, setQueue] = useState<QueuedTurn[]>([]);
  /// L1 amount a create/join attempt needed but no single UTXO covered;
  /// drives the KeyBar "fund me" hint until the polls say it is covered.
  const [fundNeed, setFundNeed] = useState<bigint | null>(null);
  const da = useDa();
  const balances = useMyBalances(identity);
  const myL1 = balances.utxos?.reduce((sum, u) => sum + u.amount, 0n) ?? null;
  const laneCarriers = useLaneCarriers();
  const onIdentity = useCallback((id: Identity | null) => setIdentity(id), []);
  const myUserId = identity?.userIdHex ?? '';
  /// Unwitnessed rows (the T7 create/join calls) default to the my-games
  /// count witness: both entries land when a game of mine appears.
  const onActivity = (label: string, txid: string, witness?: ActivityWitness) => {
    const w = witness ?? { kind: 'myGames' as const, before: myGamesCount(da.games, myUserId) };
    setActivity((rows) => [{ id: nextActivityId++, label, txid, at: Date.now(), status: 'pending' as const, witness: w }, ...rows].slice(0, 50));
  };
  const onNeedsFunding = useCallback((needed: bigint) => setFundNeed(needed), []);
  const needsFunding = fundNeed !== null && balances.utxos !== null && pickUtxo(balances.utxos, fundNeed) === null;

  // Trust-chip walk: each DA poll advances rows whose witnessed change landed
  // and rows whose settlement moved; a no-op walk keeps the array reference.
  useEffect(() => {
    setActivity((rows) =>
      advanceActivity(rows, {
        games: da.games,
        settledTxid: da.state?.settled?.txid ?? null,
        myUserId,
        myBalance: balances.l2,
        myL1,
      }),
    );
  }, [da, myUserId, balances.l2, balances.utxos]);

  /// One of my turn carriers for the selected game is still in flight: the
  /// board must not accept another click until it lands (the old board let a
  /// second click double-submit into a guaranteed rejection).
  const myTurnInFlight =
    selectedGame !== null && activity.some((r) => r.status === 'pending' && r.witness?.kind === 'board' && r.witness.gameId === selectedGame);

  const onEnqueue = useCallback((gameId: string, cell: number) => {
    setQueue((q) => [...q, { id: nextQueueId++, gameId, cell }]);
  }, []);
  const onCancelQueued = useCallback((id: number) => {
    setQueue((q) => q.filter((e) => e.id !== id));
  }, []);

  // Premove dispatcher: for each game, the oldest queued turn fires once it is
  // legal (my move, empty cell, nothing in flight for the game — an opponent
  // carrier in the L1 queue would flip the parity under ours). Stale heads
  // (cell taken, game over) drop out of the queue instead.
  const dispatching = useRef(false);
  useEffect(() => {
    if (!identity || dispatching.current) return;
    const all = [...da.games.open, ...da.games.playing, ...da.games.finished];
    const heads = new Map<string, QueuedTurn>();
    const drop: number[] = [];
    for (const entry of queue) {
      const game = all.find((g) => g.id === entry.gameId);
      if (!game) continue;
      if (game.state >= 2 || game.board[entry.cell] !== 0) {
        drop.push(entry.id);
        continue;
      }
      if (!heads.has(entry.gameId)) heads.set(entry.gameId, entry);
    }
    if (drop.length > 0) setQueue((q) => q.filter((e) => !drop.includes(e.id)));
    if (!da.state?.lane_subnet) return;
    for (const [gameId, head] of heads) {
      const game = all.find((g) => g.id === gameId)!;
      const mineInFlight = activity.some((r) => r.status === 'pending' && r.witness?.kind === 'board' && r.witness.gameId === gameId);
      const anyInFlight = inFlightTurns(laneCarriers, gameId).length > 0;
      if (!turnDispatchable(game, myUserId, mineInFlight, anyInFlight, head.cell)) continue;
      dispatching.current = true;
      setQueue((q) => q.filter((e) => e.id !== head.id));
      void (async () => {
        try {
          const client = await sharedClient();
          await submitTurn({ identity, client, lane: da.state!.lane_subnet!, game, cell: head.cell, onActivity, onNeedsFunding });
        } catch (e) {
          console.warn('premove dispatch failed', e);
        } finally {
          dispatching.current = false;
        }
      })();
      break; // one dispatch per poll is plenty
    }
  }, [da, queue, identity, activity, laneCarriers]);

  return (
    <>
      <KeyBar onIdentity={onIdentity} l1={myL1} needsFunding={needsFunding} />
      {identity && <SettlementBanner identity={identity} />}
      {identity && <LaneQueue />}
      {!da.reachable && <div className="banner">DA server unreachable — retrying…</div>}
      {identity && (
        <div className="cols">
          <section aria-label="open games">
            <OpenGames identity={identity} balances={balances} onActivity={onActivity} onNeedsFunding={onNeedsFunding} />
            <CreatePanel identity={identity} balances={balances} onActivity={onActivity} onNeedsFunding={onNeedsFunding} />
            <MyGames identity={identity} selectedId={selectedGame} onSelect={setSelectedGame} />
          </section>
          <section aria-label="match">
            <MatchPanel
              identity={identity}
              gameId={selectedGame}
              myTurnInFlight={myTurnInFlight}
              queue={queue.filter((q) => q.gameId === selectedGame)}
              onEnqueue={onEnqueue}
              onCancelQueued={onCancelQueued}
              onActivity={onActivity}
              onNeedsFunding={onNeedsFunding}
            />
          </section>
          <section aria-label="actions">
            <TransferForm identity={identity} balances={balances} onActivity={onActivity} onNeedsFunding={onNeedsFunding} />
            <WithdrawForm identity={identity} balances={balances} onActivity={onActivity} onNeedsFunding={onNeedsFunding} />
            <ClaimExits identity={identity} balances={balances} onActivity={onActivity} />
            <ActivityLog rows={activity} />
          </section>
        </div>
      )}
    </>
  );
}
