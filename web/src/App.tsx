//! One page: KeyBar, settlement banner, three columns (game lists, match,
//! actions + activity). Rows enter the activity log `pending` at submit and
//! the DA poll walks their trust chips via match.ts `advanceActivity`.

import { useCallback, useEffect, useState } from 'react';
import { pickUtxo, useMyBalances, type ActivityRow } from './composition';
import { ActivityLog } from './ActivityLog';
import { ClaimExits } from './ClaimExits';
import { KeyBar, type Identity } from './KeyBar';
import { CreatePanel } from './CreatePanel';
import { MatchPanel } from './MatchPanel';
import { MyGames } from './MyGames';
import { OpenGames } from './OpenGames';
import { SettlementBanner } from './SettlementBanner';
import { TransferForm } from './TransferForm';
import { WithdrawForm } from './WithdrawForm';
import { advanceActivity, myGamesCount, type ActivityWitness } from './match';
import { useDa } from './state';

export default function App() {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [selectedGame, setSelectedGame] = useState<string | null>(null);
  const [activity, setActivity] = useState<ActivityRow[]>([]);
  /// L1 amount a create/join attempt needed but no single UTXO covered;
  /// drives the KeyBar "fund me" hint until the polls say it is covered.
  const [fundNeed, setFundNeed] = useState<bigint | null>(null);
  const da = useDa();
  const balances = useMyBalances(identity);
  const onIdentity = useCallback((id: Identity | null) => setIdentity(id), []);
  const myUserId = identity?.userIdHex ?? '';
  /// Unwitnessed rows (the T7 create/join calls) default to the my-games
  /// count witness: both entries land when a game of mine appears.
  const onActivity = (label: string, txid: string, witness?: ActivityWitness) => {
    const w = witness ?? { kind: 'myGames' as const, before: myGamesCount(da.games, myUserId) };
    setActivity((rows) => [{ id: rows.length, label, txid, status: 'pending' as const, witness: w }, ...rows].slice(0, 50));
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
        myL1: balances.utxos?.reduce((sum, u) => sum + u.amount, 0n) ?? null,
      }),
    );
  }, [da, myUserId, balances.l2, balances.utxos]);

  return (
    <>
      <KeyBar onIdentity={onIdentity} needsFunding={needsFunding} />
      {identity && <SettlementBanner identity={identity} />}
      {!da.reachable && <div className="banner">DA server unreachable — retrying…</div>}
      {identity && (
        <div className="cols">
          <section aria-label="open games">
            <OpenGames identity={identity} balances={balances} onActivity={onActivity} onNeedsFunding={onNeedsFunding} />
            <CreatePanel identity={identity} balances={balances} onActivity={onActivity} onNeedsFunding={onNeedsFunding} />
            <MyGames identity={identity} selectedId={selectedGame} onSelect={setSelectedGame} />
          </section>
          <section aria-label="match">
            <MatchPanel identity={identity} gameId={selectedGame} onActivity={onActivity} onNeedsFunding={onNeedsFunding} />
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
