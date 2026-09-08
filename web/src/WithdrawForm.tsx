//! Withdraw form: amount only — the exit destination is fixed to the caller's
//! own schnorr pubkey inside the encoder, shown read-only, never an input.
//! The amount floors at the config `min_withdrawal_amount`; an uninitialized
//! config disables the form.

import { useState } from 'react';
import { FEE_ESTIMATE, canAfford, kas, sharedClient, shortHex, submitWithdraw, type MyBalances } from './composition';
import type { Identity } from './KeyBar';
import type { ActivityWitness } from './match';
import { useDa } from './state';
import { meetsWithdrawFloor, parseSompi } from './transfer';

export function WithdrawForm({
  identity,
  balances,
  onActivity,
  onNeedsFunding,
}: {
  identity: Identity;
  balances: MyBalances;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
  onNeedsFunding: (needed: bigint) => void;
}) {
  const da = useDa();
  const [amountKas, setAmountKas] = useState('');
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const initialized = da.config?.initialized === true;
  const min = da.config?.min_withdrawal_amount;
  const amount = parseSompi(amountKas);
  const floorOk = amount !== null && meetsWithdrawFloor(amount, min);
  const l2Covers = balances.l2 === null || (amount !== null && amount <= balances.l2);
  const afford = balances.utxos !== null && canAfford(balances.utxos, 0n, FEE_ESTIMATE);

  const withdraw = async () => {
    setErr(null);
    setDone(null);
    if (amount === null) {
      setErr('amount must be a positive KAS amount');
      return;
    }
    if (!floorOk) {
      setErr(`minimum withdrawal is ${kas(min!)}`);
      return;
    }
    const lane = da.state?.lane_subnet;
    if (!lane) {
      setErr('waiting for DA state');
      return;
    }
    setBusy(true);
    try {
      const txid = await submitWithdraw({
        identity,
        client: await sharedClient(),
        lane,
        amount,
        balanceBefore: balances.l2,
        onActivity,
        onNeedsFunding,
      });
      setDone(`submitted ${shortHex(txid)} · ${kas(amount)} → ${identity.wallet.address}`);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      <h3>withdraw</h3>
      {!initialized && <p className="hint">not initialized — waiting for the DA config</p>}
      <div className="card">
        <label>
          amount <input value={amountKas} onChange={(e) => setAmountKas(e.target.value)} inputMode="decimal" size={6} /> KAS
        </label>
        {min !== undefined && <span className="hint">min withdrawal {kas(min)}</span>}
        <span className="hint" title={identity.wallet.pubkeyHex}>
          dest: mine — {identity.wallet.address} (key {shortHex(identity.wallet.pubkeyHex)})
        </span>
        <button disabled={!initialized || busy || !floorOk || !l2Covers || !afford} onClick={withdraw}>
          {busy ? 'withdrawing…' : 'Withdraw'}
        </button>
      </div>
      {err && <div className="err">{err}</div>}
      {done && <div className="ok">{done}</div>}
    </div>
  );
}
