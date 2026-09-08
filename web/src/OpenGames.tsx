//! Open games column: one card per open game with an affordability-gated Join.

import { useState } from 'react';
import {
  FEE_ESTIMATE,
  MIN_CREATE_BALANCE,
  canAfford,
  covenantIdOf,
  entryDeposit,
  kas,
  sharedClient,
  shortHex,
  submitEntry,
  type MyBalances,
} from './composition';
import type { Identity } from './KeyBar';
import { useDa } from './state';

export function OpenGames({
  identity,
  balances,
  onActivity,
  onNeedsFunding,
}: {
  identity: Identity;
  balances: MyBalances;
  onActivity: (label: string, txid: string) => void;
  onNeedsFunding: (needed: bigint) => void;
}) {
  const da = useDa();
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const join = async (gameId: string, stake: bigint) => {
    setErr(null);
    const lane = da.state?.lane_subnet;
    const covenantId = covenantIdOf(da.config?.covenant_id, da.state?.covenant_id);
    if (!lane || !covenantId) {
      setErr('waiting for DA state');
      return;
    }
    setBusy(gameId);
    try {
      await submitEntry({
        identity,
        client: await sharedClient(),
        lane,
        covenantId,
        entry: { kind: 'join', stake, gameId },
        onActivity,
        onNeedsFunding,
      });
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="stack">
      <h3>open games</h3>
      {err && <div className="err">{err}</div>}
      {da.games.open.length === 0 && <p className="hint">no open games — create one below</p>}
      {da.games.open.map((g) => {
        const stake = BigInt(g.stake);
        // Join stays enabled when the L2 balance covers the stake, or when a
        // single L1 UTXO affords the auto-deposit; unknown balances cannot gate.
        const shortL2 = balances.l2 !== null && balances.l2 < stake;
        const l1Covers =
          balances.utxos !== null &&
          canAfford(
            balances.utxos,
            entryDeposit({
              exists: balances.exists,
              balance: balances.l2 ?? 0n,
              stake,
              minCreateBalance: MIN_CREATE_BALANCE,
            }),
            FEE_ESTIMATE,
          );
        const disabled = busy !== null || (shortL2 && !l1Covers);
        return (
          <div key={g.id} className="card">
            <span>stake {kas(g.stake)}</span>
            <span>rounds {g.rounds_total}</span>
            <span>by {shortHex(g.players[0])}</span>
            <button disabled={disabled} onClick={() => join(g.id, stake)} title={disabled ? 'balance too low for stake and auto-deposit' : undefined}>
              {busy === g.id ? 'joining…' : 'Join'}
            </button>
          </div>
        );
      })}
    </div>
  );
}
