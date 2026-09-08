//! One page: KeyBar, settlement banner, three columns. Columns two (match)
//! and the actions half of column three land in later tasks.

import { useCallback, useState } from 'react';
import { useMyBalances, shortHex, type ActivityRow } from './composition';
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
  const da = useDa();
  const balances = useMyBalances(identity);
  const onIdentity = useCallback((id: Identity | null) => setIdentity(id), []);
  const onActivity = useCallback(
    (label: string, txid: string) =>
      setActivity((rows) => [{ id: rows.length, label, txid, status: 'pending' as const }, ...rows].slice(0, 50)),
    [],
  );

  return (
    <>
      <KeyBar onIdentity={onIdentity} />
      {identity && <SettlementBanner identity={identity} />}
      {!da.reachable && <div className="banner">DA server unreachable — retrying…</div>}
      {identity && (
        <div className="cols">
          <section aria-label="open games">
            <OpenGames identity={identity} balances={balances} onActivity={onActivity} />
            <CreatePanel identity={identity} balances={balances} onActivity={onActivity} />
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
