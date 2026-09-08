//! One page: KeyBar, settlement banner, three columns. Columns two (match)
//! and the actions half of column three land in later tasks.

import { useCallback, useState } from 'react';
import { pickUtxo, useMyBalances, shortHex, type ActivityRow } from './composition';
import { KeyBar, type Identity } from './KeyBar';
import { CreatePanel } from './CreatePanel';
import { MyGames } from './MyGames';
import { OpenGames } from './OpenGames';
import { SettlementBanner } from './SettlementBanner';
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
  const onActivity = useCallback(
    (label: string, txid: string) =>
      setActivity((rows) => [{ id: rows.length, label, txid, status: 'pending' as const }, ...rows].slice(0, 50)),
    [],
  );
  const onNeedsFunding = useCallback((needed: bigint) => setFundNeed(needed), []);
  const needsFunding = fundNeed !== null && balances.utxos !== null && pickUtxo(balances.utxos, fundNeed) === null;

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
            <p className="hint">{selectedGame ? 'match panel lands in the next task' : 'select a game in my games'}</p>
          </section>
          <section aria-label="actions">
            <div className="stack">
              <h3>activity</h3>
              {activity.length === 0 && <p className="hint">nothing submitted yet</p>}
              <ul className="activity">
                {activity.map((r) => (
                  <li key={r.id}>
                    {r.label} · {shortHex(r.txid)} <span className={`chip ${r.status === 'pending' ? 'pending' : 'done'}`}>{r.status}</span>
                  </li>
                ))}
              </ul>
            </div>
          </section>
        </div>
      )}
    </>
  );
}
