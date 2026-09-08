//! Create panel: stake, rounds, mark; submits one carrier whose payload is
//! `[Deposit?] + CreateGame` with the auto-sized entry deposit.

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

const SOMPI = 100_000_000;

function parseStake(kas: string): bigint | null {
  const n = Number(kas);
  if (!Number.isFinite(n) || n <= 0) return null;
  return BigInt(Math.round(n * SOMPI));
}

function parseRounds(rounds: string): number | null {
  const n = Number(rounds);
  return Number.isInteger(n) && n >= 1 && n <= 99 ? n : null;
}

export function CreatePanel({
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
  const [stakeKas, setStakeKas] = useState('0.5');
  const [rounds, setRounds] = useState('3');
  const [mark, setMark] = useState<1 | 2>(1);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const initialized = da.config?.initialized === true;

  const create = async () => {
    const stake = parseStake(stakeKas);
    const n = parseRounds(rounds);
    if (stake === null) {
      setErr('stake must be a positive KAS amount');
      return;
    }
    if (n === null) {
      setErr('rounds must be an integer 1..99');
      return;
    }
    setErr(null);
    setDone(null);
    const lane = da.state?.lane_subnet;
    const covenantId = covenantIdOf(da.config?.covenant_id, da.state?.covenant_id);
    if (!lane || !covenantId) {
      setErr('waiting for DA state');
      return;
    }
    setBusy(true);
    try {
      const res = await submitEntry({
        identity,
        client: await sharedClient(),
        lane,
        covenantId,
        entry: { kind: 'create', stake, rounds: n, mark },
        onActivity,
        onNeedsFunding,
      });
      // Deposit confirmation line: the covenant (P2SH) deposit address, only
      // when this entry actually carried a deposit.
      setDone(
        res.deposit > 0n
          ? `submitted ${shortHex(res.txid)} · entry deposit ${kas(res.deposit)} → ${da.state?.deposit_address}`
          : `submitted ${shortHex(res.txid)}`,
      );
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const stake = parseStake(stakeKas);
  const deposit = stake === null ? null : entryDeposit({ exists: balances.exists, balance: balances.l2 ?? 0n, stake, minCreateBalance: MIN_CREATE_BALANCE });
  const afford = deposit !== null && balances.utxos !== null && canAfford(balances.utxos, deposit, FEE_ESTIMATE);

  return (
    <div className="stack">
      <h3>create</h3>
      {!initialized && <p className="hint">not initialized — waiting for the DA config</p>}
      <div className="card">
        <label>
          stake{' '}
          <input value={stakeKas} onChange={(e) => setStakeKas(e.target.value)} inputMode="decimal" size={6} /> KAS
        </label>
        <label>
          rounds <input value={rounds} onChange={(e) => setRounds(e.target.value)} inputMode="numeric" size={3} /></label>
        <span>
          mark
          <label>
            <input type="radio" checked={mark === 1} onChange={() => setMark(1)} /> X
          </label>
          <label>
            <input type="radio" checked={mark === 2} onChange={() => setMark(2)} /> O
          </label>
        </span>
        {initialized && deposit !== null && deposit > 0n && (
          <span className="hint">
            entry deposit {kas(deposit)} → {da.state?.deposit_address}
          </span>
        )}
        <button disabled={!initialized || busy || (deposit !== null && !afford)} onClick={create}>
          {busy ? 'creating…' : 'Create game'}
        </button>
      </div>
      {err && <div className="err">{err}</div>}
      {done && <div className="ok">{done}</div>}
    </div>
  );
}
