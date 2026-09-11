//! KeyBar: privkey input (memory only), derived address + copy, L2/L1 balances.

import { useEffect, useState } from 'react';
import { fetchAccount } from './da';
import { useDa } from './state';
import { connectClient, initKaspa, loadKey, NETWORK, type Wallet } from './wallet';
import initEncoder, { my_ids } from 'vprog-tictactoe-encoder-wasm';
import type { RpcClient } from 'kaspa-wasm';

let encoderInit: Promise<unknown> | null = null;

export interface Identity {
  wallet: Wallet;
  /// The key hex itself (memory only, never persisted): the encoder builders
  /// sign from the private key, not from the kaspa-wasm keypair.
  privkeyHex: string;
  /// `my_ids()` view of the same key: lock hash, user resource id, x-only pubkey.
  lockHashHex: string;
  userIdHex: string;
}

/// Loads encoder + kaspa wasm modules and derives the identity for `hex`.
export async function loadIdentity(privkeyHex: string): Promise<Identity> {
  encoderInit ??= initEncoder();
  await Promise.all([encoderInit, initKaspa()]);
  const ids = my_ids(privkeyHex);
  return {
    wallet: loadKey(privkeyHex),
    privkeyHex,
    lockHashHex: ids.lock_hash_hex,
    userIdHex: ids.user_id_hex,
  };
}

const SOMPI = 100_000_000n;
function kas(amount: bigint): string {
  return `${(Number(amount) / Number(SOMPI)).toFixed(2)} KAS`;
}

export function KeyBar({ onIdentity, needsFunding = false }: { onIdentity: (id: Identity | null) => void; needsFunding?: boolean }) {
  const [privkey, setPrivkey] = useState('');
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [client, setClient] = useState<RpcClient | null>(null);
  const [l1, setL1] = useState<bigint | null>(null);
  const [l2, setL2] = useState<bigint | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const da = useDa();

  useEffect(() => onIdentity(identity), [identity, onIdentity]);

  // L2 balance follows the 2 s DA poll; L1 follows a matching local poll.
  useEffect(() => {
    if (!identity) {
      setL2(null);
      return;
    }
    let stop = false;
    const tick = () =>
      fetchAccount(identity.userIdHex)
        .then((a) => !stop && setL2(a.exists && a.balance !== undefined ? BigInt(a.balance) : 0n))
        .catch(() => !stop && setL2(null));
    tick();
    const id = setInterval(tick, 2_000);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [identity]);

  useEffect(() => {
    if (!identity || !client) {
      setL1(null);
      return;
    }
    let stop = false;
    const tick = () =>
      identity.wallet
        .l1Utxos(client)
        .then((utxos) => !stop && setL1(utxos.reduce((sum, u) => sum + u.amount, 0n)))
        .catch(() => !stop && setL1(null));
    tick();
    const id = setInterval(tick, 2_000);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [identity, client]);

  const load = async () => {
    setErr(null);
    try {
      const id = await loadIdentity(privkey.trim());
      setIdentity(id);
      setPrivkey('');
      // L1 connect is gated separately: an L1-down at key-load time must not
      // tear the identity down — surface the error and keep the key loaded.
      connectClient()
        .then(setClient)
        .catch((e) => setErr(`L1 connect failed: ${String(e)}`));
    } catch (e) {
      setIdentity(null);
      setClient(null);
      setErr(String(e));
    }
  };

  const copy = async () => {
    if (!identity) return;
    await navigator.clipboard.writeText(identity.wallet.address);
    setCopied(true);
    setTimeout(() => setCopied(false), 1_500);
  };

  return (
    <div className="keybar">
      <input
        type="password"
        placeholder="privkey hex (never stored)"
        value={privkey}
        onChange={(e) => setPrivkey(e.target.value)}
        onKeyDown={(e) => e.key === 'Enter' && load()}
      />
      <button onClick={load}>Load</button>
      {identity && (
        <>
          <span className="addr" title="this address must receive L1 funds">
            addr: {identity.wallet.address}{' '}
          </span>
          <button onClick={copy}>{copied ? 'copied' : 'copy'}</button>
          <span>L2: {l2 === null ? '…' : kas(l2)}</span>
          <span>L1: {l1 === null ? '…' : kas(l1)}</span>
          {needsFunding && (
            <span className="fundme" title="this address must receive L1 funds">
              fund me: {identity.wallet.address}
            </span>
          )}
          <span>[{NETWORK}{da.reachable ? '' : ' · DA unreachable'}]</span>
        </>
      )}
      {err && <span style={{ color: 'crimson' }}>{err}</span>}
    </div>
  );
}
