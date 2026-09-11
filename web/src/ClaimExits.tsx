//! Claim exits: one row per settled exit leaf paying my own schnorr key.
//! Every claim is a full-leaf deduct (the demo lock — no partial claims),
//! funded by the delegate UTXOs at the covenant deposit address; the payout
//! lands on L1 and the activity row acks on my L1 sum rising. A submitted
//! claim keeps its row showing the pending chip until the DA marks the leaf
//! spent and the filter drops it.

import { useState } from 'react';
import { kas, sharedClient, submitClaim, type MyBalances } from './composition';
import type { ExitLeaf, ExitRoot } from './da';
import type { Identity } from './KeyBar';
import type { ActivityWitness } from './match';
import { claimableExits } from './claim';
import { useDa } from './state';

export function ClaimExits({
  identity,
  balances,
  onActivity,
}: {
  identity: Identity;
  balances: MyBalances;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
}) {
  const da = useDa();
  const [busyKey, setBusyKey] = useState<string | null>(null);
  /// `root:index` keys of claims already submitted — the ⏸pending state.
  const [claimed, setClaimed] = useState<Set<string>>(() => new Set());
  const [err, setErr] = useState<string | null>(null);

  const targets = claimableExits(da.exits, identity.wallet.pubkeyHex);
  const l1Before = balances.utxos?.reduce((sum, u) => sum + u.amount, 0n) ?? null;

  const claim = async (key: string, root: ExitRoot, leaf: ExitLeaf) => {
    const covenantId = da.state?.covenant_id;
    const depositAddress = da.state?.deposit_address;
    if (!covenantId || !depositAddress) {
      setErr('waiting for DA state');
      return;
    }
    setErr(null);
    setBusyKey(key);
    try {
      await submitClaim({
        identity,
        client: await sharedClient(),
        covenantId,
        depositAddress,
        root,
        leaf,
        balanceBefore: l1Before,
        onActivity,
      });
      setClaimed((prev) => new Set(prev).add(key));
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  return (
    <div className="stack">
      <h3>claim exits</h3>
      {targets.length === 0 && <p className="hint">no settled exits paying to my key</p>}
      {targets.map((t) => {
        const pending = claimed.has(t.key);
        return (
          <div className="card" key={t.key}>
            <span>{kas(t.leaf.amount)}</span>
            <button disabled={pending || busyKey !== null} onClick={() => claim(t.key, t.root, t.leaf)}>
              Claim
            </button>
            <span className={`chip ${pending ? 'pending' : 'settled'}`}>{pending ? '⏸pending' : '⚲settled'}</span>
          </div>
        );
      })}
      {err && <div className="err">{err}</div>}
    </div>
  );
}
