//! Transfer form: dest user-resource id + amount; a dest pubkey input appears
//! only when the account does not exist yet (the create branch, whose pubkey
//! must derive the typed dest id). Change-only carrier over the shared submit
//! pipeline, acked by the balance witness.

import { useEffect, useState } from 'react';
import { FEE_ESTIMATE, canAfford, kas, sharedClient, shortHex, submitTransfer, type MyBalances } from './composition';
import { fetchAccount } from './da';
import type { Identity } from './KeyBar';
import type { ActivityWitness } from './match';
import { useDa } from './state';
import { destUserResource, parseSompi } from './transfer';

const HEX64 = /^[0-9a-fA-F]{64}$/;

export function TransferForm({
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
  const [destId, setDestId] = useState('');
  const [destPub, setDestPub] = useState('');
  const [amountKas, setAmountKas] = useState('');
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  /// null until the typed id resolves against /api/accounts; true once the
  /// account is known missing — the create branch needing the dest pubkey.
  const [destMissing, setDestMissing] = useState<boolean | null>(null);

  const id = destId.trim();
  const pubkey = destPub.trim();

  // Resolve the dest account once the typed id is a full resource id; drives
  // the pubkey input without a submit.
  useEffect(() => {
    if (!HEX64.test(id)) {
      setDestMissing(null);
      return;
    }
    let stop = false;
    const t = setTimeout(() => {
      fetchAccount(id)
        .then((a) => !stop && setDestMissing(!a.exists))
        .catch(() => !stop && setDestMissing(null));
    }, 400);
    return () => {
      stop = true;
      clearTimeout(t);
    };
  }, [id]);

  const amount = parseSompi(amountKas);
  const l2Covers = balances.l2 === null || (amount !== null && amount <= balances.l2);
  const afford = balances.utxos !== null && canAfford(balances.utxos, 0n, FEE_ESTIMATE);
  const needPub = destMissing === true;

  const transfer = async () => {
    setErr(null);
    setDone(null);
    if (!HEX64.test(id)) {
      setErr('dest must be a 64-hex user resource id');
      return;
    }
    if (destMissing === null) {
      setErr('checking destination account…');
      return;
    }
    if (amount === null) {
      setErr('amount must be a positive KAS amount');
      return;
    }
    if (id.toLowerCase() === identity.userIdHex) {
      setErr('dest is your own account');
      return;
    }
    if (needPub) {
      if (!HEX64.test(pubkey)) {
        setErr('dest pubkey (64 hex) required — the account does not exist yet');
        return;
      }
      if ((await destUserResource(pubkey)) !== id.toLowerCase()) {
        setErr('dest pubkey does not derive this user id');
        return;
      }
    }
    const lane = da.state?.lane_subnet;
    if (!lane) {
      setErr('waiting for DA state');
      return;
    }
    setBusy(true);
    try {
      const txid = await submitTransfer({
        identity,
        client: await sharedClient(),
        lane,
        destUserIdHex: id.toLowerCase(),
        destExists: !needPub,
        destPubkeyHex: needPub ? pubkey.toLowerCase() : undefined,
        amount,
        balanceBefore: balances.l2,
        onActivity,
        onNeedsFunding,
      });
      setDone(`submitted ${shortHex(txid)} · ${kas(amount)} → ${shortHex(id)}`);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      <h3>transfer</h3>
      <div className="card">
        <label>
          dest <input value={destId} onChange={(e) => setDestId(e.target.value)} placeholder="user resource id hex" size={38} />
        </label>
        {needPub && (
          <label>
            dest pubkey{' '}
            <input
              value={destPub}
              onChange={(e) => setDestPub(e.target.value)}
              placeholder="x-only pubkey (account missing — will be created)"
              size={38}
            />
          </label>
        )}
        {destMissing === false && <span className="hint">dest account exists</span>}
        <label>
          amount <input value={amountKas} onChange={(e) => setAmountKas(e.target.value)} inputMode="decimal" size={6} /> KAS
        </label>
        <button disabled={busy || !afford || !l2Covers} onClick={transfer}>
          {busy ? 'sending…' : 'Transfer'}
        </button>
      </div>
      {err && <div className="err">{err}</div>}
      {done && <div className="ok">{done}</div>}
    </div>
  );
}
