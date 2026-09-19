//! Lane-carrier decoding: reads the payload wire the encoder builds (app-kit
//! `LanePayload`) just far enough to name each action. The L1 queue strip and
//! the board's pending-turn ghosts render from this; rejections stay invisible
//! everywhere (see the activity log's `no L2 effect` chip for those).
//!
//! Wire layout (app-kit payload.rs + guest encode.rs, pinned by
//! __tests__/carriers.test.ts against the real encoder wasm):
//!   access  : u32 count || count × (32B resource id, 1B access type)
//!   signers : u32 count || count × entry (res_idx u8, kind tag u8, kind fields)
//!   actions : u32 count || count × (tag u8 || body), bodies concatenated

import { createContext, createElement, useContext, useEffect, useState, type ReactNode } from 'react';
import { Transaction } from 'kaspa-wasm';
import { sharedClient } from './composition';
import { useDa } from './state';

const POLL_MS = 4_000;
/// How long a failing poll's last-good in-flight view may keep gating the
/// premove dispatcher before it is dropped as stale.
const STALE_CARRIERS_MS = 15_000;

/// One decoded action, resource ids resolved through the payload's access list.
export type LaneAction =
  | { kind: 'turn'; gameId: string; userId: string; cell: number }
  | { kind: 'create'; creatorId: string; gameId: string; stake: bigint; rounds: number; mark: number }
  | { kind: 'join'; gameId: string; joinerId: string }
  | { kind: 'deposit'; userId: string }
  | { kind: 'withdraw'; userId: string; amount: bigint }
  | { kind: 'transfer'; sourceId: string; destId: string; amount: bigint; createsDest: boolean }
  | { kind: 'other'; tag: number };

/// One in-flight lane carrier: its L1 txid plus the decoded actions.
export interface LaneCarrier {
  txid: string;
  actions: LaneAction[];
}

// Action tags (guest program/action.rs `ActionTag`).
const TAG_UPDATE = 0x01;
const TAG_INIT = 0x02;
const TAG_TRANSFER = 0x03;
const TAG_UPDATE_USER_LOCK = 0x04;
const TAG_DEPOSIT = 0x05;
const TAG_WITHDRAW = 0x06;
const TAG_CREATE_GAME = 0x07;
const TAG_JOIN_GAME = 0x08;
const TAG_TURN = 0x09;
const TAG_TIMEOUT = 0x0a;

// Lock tags (runtime lock_variants): Schnorr 32B body, Multisig 2 + 32n, Unlocked empty.
const LOCK_SCHNORR = 0x01;
const LOCK_MULTISIG = 0x02;
// StandardSpk tags (zk abi withdrawal): 0 PubKey 32B, 1 PubKeyEcdsa 33B, 8 ScriptHash 32B.

const HEX = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, '0'));
const hex = (b: Uint8Array): string => {
  let s = '';
  for (const v of b) s += HEX[v];
  return s;
};

class Reader {
  pos = 0;
  constructor(readonly b: Uint8Array) {}
  u8(): number {
    return this.b[this.pos++]!;
  }
  u32(): number {
    const v = (this.b[this.pos]! | (this.b[this.pos + 1]! << 8) | (this.b[this.pos + 2]! << 16) | (this.b[this.pos + 3]! << 24)) >>> 0;
    this.pos += 4;
    return v;
  }
  u64(): bigint {
    return BigInt(this.u32()) + 0x1_0000_0000n * BigInt(this.u32());
  }
  id(): string {
    const v = this.b.subarray(this.pos, this.pos + 32);
    if (v.length !== 32) throw new Error('carrier: truncated resource id');
    this.pos += 32;
    return hex(v);
  }
}

/// Skips one `LockEnum` (tag + body), advancing `r`.
function skipLock(r: Reader): void {
  const tag = r.u8();
  if (tag === LOCK_SCHNORR) r.pos += 32;
  else if (tag === LOCK_MULTISIG) r.pos += 2 + 32 * r.b[r.pos + 1]!;
}

/// Skips one `StandardSpk` (tag implies payload length).
function skipSpk(r: Reader): void {
  const tag = r.u8();
  r.pos += tag === 1 ? 33 : 32;
}

/// Decodes one lane payload (hex, as the wasm `ITransaction.payload` carries
/// it) into named actions. Throws on any structural surprise — callers treat a
/// failed decode as an undecodable carrier and show it as `other`.
export function decodeLanePayload(payloadHex: string): LaneAction[] {
  const raw = new Uint8Array(payloadHex.length / 2);
  for (let i = 0; i < raw.length; i++) raw[i] = parseInt(payloadHex.slice(2 * i, 2 * i + 2), 16);
  const r = new Reader(raw);

  const accessCount = r.u32();
  const access: string[] = [];
  for (let i = 0; i < accessCount; i++) {
    access.push(r.id());
    r.u8();
  }

  const signerCount = r.u32();
  for (let i = 0; i < signerCount; i++) {
    r.u8(); // resource idx
    const tag = r.u8();
    if (tag === 0x03) r.u8(); // multisig pubkey_idx
    if (tag === 0x02 || tag === 0x04) {
      // witness kinds: input_idx + three u32 offsets
      r.u8();
      r.pos += 12;
    } else {
      r.pos += 4; // sig_offset
    }
  }

  const actionCount = r.u32();
  const actions: LaneAction[] = [];
  for (let i = 0; i < actionCount; i++) {
    const tag = r.u8();
    if (tag === TAG_TURN) {
      actions.push({ kind: 'turn', gameId: access[r.u8()]!, userId: access[r.u8()]!, cell: r.u8() });
    } else if (tag === TAG_CREATE_GAME) {
      const creatorId = access[r.u8()]!;
      // The newborn game's resource id rides in the access list (the encoder
      // derives it from the creator's lock and games-started counter).
      const gameId = access[r.u8()]!;
      const stake = r.u64();
      actions.push({ kind: 'create', creatorId, gameId, stake, rounds: r.u8(), mark: r.u8() });
    } else if (tag === TAG_JOIN_GAME) {
      actions.push({ kind: 'join', gameId: access[r.u8()]!, joinerId: access[r.u8()]! });
    } else if (tag === TAG_DEPOSIT) {
      const userId = access[r.u8()]!;
      r.u8(); // config_idx
      r.u32(); // output_idx
      skipLock(r);
      actions.push({ kind: 'deposit', userId });
    } else if (tag === TAG_WITHDRAW) {
      const userId = access[r.u8()]!;
      r.u8(); // config_idx
      const amount = r.u64();
      skipSpk(r);
      actions.push({ kind: 'withdraw', userId, amount });
    } else if (tag === TAG_TRANSFER) {
      const sourceId = access[r.u8()]!;
      const destId = access[r.u8()]!;
      const amount = r.u64();
      const createsDest = r.u8() === 1;
      if (createsDest) skipLock(r);
      actions.push({ kind: 'transfer', sourceId, destId, amount, createsDest });
    } else if (tag === TAG_UPDATE || tag === TAG_INIT) {
      r.u8();
      r.u64();
      r.u64();
      r.pos += 32; // covenant id
      skipLock(r);
      actions.push({ kind: 'other', tag });
    } else if (tag === TAG_UPDATE_USER_LOCK) {
      r.u8();
      skipLock(r);
      actions.push({ kind: 'other', tag });
    } else if (tag === TAG_TIMEOUT) {
      r.u8();
      r.u8();
      actions.push({ kind: 'other', tag });
    } else {
      throw new Error(`carrier: unknown action tag ${tag}`);
    }
  }
  return actions;
}

/// One-line label for a decoded action (queue strip copy).
export function actionLabel(a: LaneAction): string {
  switch (a.kind) {
    case 'turn':
      return `turn cell ${a.cell} by ${a.userId.slice(0, 6)}…`;
    case 'create':
      return `create ${Number(a.stake) / 1e8} KAS by ${a.creatorId.slice(0, 6)}…`;
    case 'join':
      return `join by ${a.joinerId.slice(0, 6)}…`;
    case 'deposit':
      return `deposit for ${a.userId.slice(0, 6)}…`;
    case 'withdraw':
      return `withdraw ${Number(a.amount) / 1e8} KAS by ${a.userId.slice(0, 6)}…`;
    case 'transfer':
      return `transfer ${Number(a.amount) / 1e8} KAS by ${a.sourceId.slice(0, 6)}…`;
    default:
      return `action ${a.tag}`;
  }
}

// ---------------------------------------------------------------------------
// Polled in-flight carriers (the lane's L1 queue), shared via context.

interface LaneCarriersSnapshot {
  /// Decoded in-flight carriers; null until the first successful poll.
  carriers: LaneCarrier[] | null;
}

const LaneCarriersContext = createContext<LaneCarriersSnapshot>({ carriers: null });

export function LaneCarriersProvider({ children }: { children: ReactNode }) {
  const da = useDa();
  const lane = da.state?.lane_subnet ?? null;
  const [carriers, setCarriers] = useState<LaneCarrier[] | null>(null);

  useEffect(() => {
    if (!lane) return;
    let stop = false;
    let running = false;
    let lastOk = 0;
    const tick = async () => {
      // One poll at a time: getMempoolEntries is the heaviest call on the
      // shared connection (the whole pool), and stacked copies of it grow an
      // unbounded queue that starves every later request into the client's
      // 60 s timeout.
      if (running) return;
      running = true;
      try {
        const client = await sharedClient();
        const r = await client.getMempoolEntries({ includeOrphanPool: false, filterTransactionPool: false });
        const decoded: LaneCarrier[] = [];
        for (const e of r.mempoolEntries) {
          if (e.transaction.subnetworkId !== lane) continue;
          const txid = new Transaction(e.transaction).id;
          try {
            decoded.push({ txid, actions: decodeLanePayload(e.transaction.payload) });
          } catch {
            decoded.push({ txid, actions: [{ kind: 'other', tag: -1 }] });
          }
        }
        lastOk = Date.now();
        if (!stop) setCarriers(decoded);
      } catch {
        // A stale in-flight view gates the premove dispatcher's parity guard
        // shut forever; an empty one only risks a parity race the guest
        // rejects at submit. Drop it once it ages out.
        if (!stop && Date.now() - lastOk > STALE_CARRIERS_MS) setCarriers([]);
      } finally {
        running = false;
      }
    };
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [lane]);

  return createElement(LaneCarriersContext.Provider, { value: { carriers } }, children);
}

/// In-flight lane carriers, refreshed every few seconds.
export function useLaneCarriers(): LaneCarrier[] {
  return useContext(LaneCarriersContext).carriers ?? [];
}

/// In-flight `Turn` actions for one game, any mover.
export function inFlightTurns(carriers: LaneCarrier[], gameId: string): { userId: string; cell: number }[] {
  const turns: { userId: string; cell: number }[] = [];
  for (const c of carriers)
    for (const a of c.actions) if (a.kind === 'turn' && a.gameId === gameId) turns.push({ userId: a.userId, cell: a.cell });
  return turns;
}
