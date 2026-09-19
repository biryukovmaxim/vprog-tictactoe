//! KeyBar: privkey input (memory only), derived address + copy, L2/L1 balances.

import { useEffect, useState } from 'react';
import { sharedClient } from './composition';
import { fetchAccount } from './da';
import { useDa } from './state';
import { initKaspa, loadKey, NETWORK, type Wallet } from './wallet';
import initEncoder, { my_ids } from 'vprog-tictactoe-encoder-wasm';

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

/// One labeled, copyable hex/address line.
function Copyable({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1_500);
  };
  return (
    <span className="addr">
      {label}: {value}{' '}
      <button onClick={copy}>{copied ? 'copied' : 'copy'}</button>
    </span>
  );
}

/// L1 total comes from App's `useMyBalances` poll; KeyBar once ran its own
/// duplicate L1 poll on a second connection, which only added load to the
/// shared node.
export function KeyBar({
  onIdentity,
  l1 = null,
  needsFunding = false,
}: {
  onIdentity: (id: Identity | null) => void;
  l1?: bigint | null;
  needsFunding?: boolean;
}) {
  const [privkey, setPrivkey] = useState('');
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [l2, setL2] = useState<bigint | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const da = useDa();

  useEffect(() => onIdentity(identity), [identity, onIdentity]);

  // L2 balance follows the 2 s DA poll.
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

  const load = async () => {
    setErr(null);
    try {
      const id = await loadIdentity(privkey.trim());
      setIdentity(id);
      setPrivkey('');
      // L1 connect is gated separately: an L1-down at key-load time must not
      // tear the identity down — surface the error and keep the key loaded.
      sharedClient().catch((e) => setErr(`L1 connect failed: ${String(e)}`));
    } catch (e) {
      setIdentity(null);
      setErr(String(e));
    }
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
          <Copyable label="addr" value={identity.wallet.address} />
          <Copyable label="id" value={identity.userIdHex} />
          <Copyable label="pk" value={identity.wallet.pubkeyHex} />
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
