//! Settlement banner: latest paired settlement state with a confirmations
//! estimate and a claim-exits shortcut. /api/state carries no confirmations
//! field (T4 ruling), so confirmations are the L1 DAA score minus the
//! settlement's DAA score.

import { useEffect, useState } from 'react';
import { shortHex, sharedClient } from './composition';
import type { Identity } from './KeyBar';
import { mySpkHex } from './claim';
import { useDa } from './state';

const POLL_MS = 2_000;

export function SettlementBanner({ identity }: { identity: Identity }) {
  const da = useDa();
  const settled = da.state?.settled ?? null;
  const [daa, setDaa] = useState<bigint | null>(null);

  // Confirmations proxy: current L1 DAA score, polled like the DA store.
  useEffect(() => {
    let stop = false;
    const tick = () =>
      sharedClient()
        .then((c) => c.getBlockDagInfo())
        .then((info) => !stop && setDaa(info.virtualDaaScore))
        .catch(() => {});
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, []);

  // Schnorr P2PK script bytes hex — the `StandardSpk::PubKey` layout exit
  // leaves carry in `spk_hex` (see claim.ts for the claimable filter).
  const mySpk = mySpkHex(identity.wallet.pubkeyHex);
  const claimable = da.exits.reduce(
    (n, root) => n + root.leaves.filter((l) => l.spk_hex === mySpk && l.spent === null).length,
    0,
  );
  const confirmations = daa !== null && settled ? daa - BigInt(settled.daa_score) : null;

  return (
    <div className="settle">
      <span className={`dot ${settled ? 'settled' : 'unsettled'}`}>●</span>
      {settled ? (
        <>
          <span>settled</span>
          <span>state {shortHex(settled.state_root)}</span>
          <span>perm {shortHex(settled.permission_root)}</span>
          <span>tx {shortHex(settled.txid)}</span>
          <span>DAA {settled.daa_score}</span>
          <span>confirmations {confirmations === null ? '…' : (confirmations < 0n ? 0n : confirmations).toString()}</span>
        </>
      ) : (
        <span>unsettled — awaiting first settlement</span>
      )}
      {claimable > 0 && <span className="claim-hint">claim exits available: {claimable}</span>}
    </div>
  );
}
