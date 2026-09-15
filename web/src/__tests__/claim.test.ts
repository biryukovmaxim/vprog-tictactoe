//! Claim-exit pins: the my-key spk filter over /api/exits leaves, the
//! full-deduct `claim_tx` argument assembly from the served shapes, the
//! delegate-pool (sum) affordance, and the submit flow over the real encoder
//! wasm (mock L1 client).

import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { submitClaim } from '../composition';
import type { ExitLeaf, ExitRoot } from '../da';
import { loadIdentity } from '../KeyBar';
import { canAffordClaim, claimArgs, claimableExits, mySpkHex } from '../claim';
import { initKaspa, type WalletUtxo } from '../wallet';
import initEncoder from 'vprog-tictactoe-encoder-wasm';
import type { RpcClient } from 'kaspa-wasm';
import { Transaction } from 'kaspa-wasm';

/// secp256k1 scalar 7 — same fixed key as the encoder crate tests.
const PRIVKEY = '0'.repeat(63) + '7';
const COVENANT = '77'.repeat(32);
const DEPOSIT = 'kaspasim:qdepositaddressmock00000000000000000';

beforeAll(async () => {
  const encoderWasm = readFileSync(createRequire(import.meta.url).resolve('vprog-tictactoe-encoder-wasm/vprog_tictactoe_encoder_wasm_bg.wasm'));
  const kaspaWasm = readFileSync(createRequire(import.meta.url).resolve('kaspa-wasm/kaspa_bg.wasm'));
  await Promise.all([initEncoder(encoderWasm), initKaspa(kaspaWasm)]);
});

afterEach(() => vi.unstubAllGlobals());

/// /api/exits fixture: one settled all-unspent root whose leaves 0 and 1 pay
/// my key, leaf 2 pays another key. The sequential-claim tests derive
/// spent-leaf and drained variants from it.
const MY_PK = '31'.repeat(32);
const THEIR_PK = '99'.repeat(32);

const FIXTURE: ExitRoot = {
  root: '11'.repeat(32),
  settlement_txid: 'aa'.repeat(32),
  outpoint_index: 1,
  daa_score: 100,
  unclaimed: 3,
  rent: 50_000_000,
  leaves: [
    {
      index: 0,
      spk_hex: mySpkHex(MY_PK),
      amount: 50_000_000,
      spent: null,
      siblings: ['22'.repeat(32), '33'.repeat(32)],
      full_claim: { new_root: '44'.repeat(32), new_unclaimed: 2 },
    },
    {
      index: 1,
      spk_hex: mySpkHex(MY_PK),
      amount: 25_000_000,
      spent: null,
      siblings: ['55'.repeat(32), '66'.repeat(32)],
      full_claim: { new_root: '77'.repeat(32), new_unclaimed: 1 },
    },
    {
      index: 2,
      spk_hex: mySpkHex(THEIR_PK),
      amount: 10_000_000,
      spent: null,
      siblings: ['88'.repeat(32), '12'.repeat(32)],
      full_claim: { new_root: '13'.repeat(32), new_unclaimed: 0 },
    },
  ],
};

/// FIXTURE after a first claim spent leaf 1: the served record has advanced
/// to the continuation (fresh root, claim txid, decremented unclaimed).
function claimedFixture(): ExitRoot {
  const spentLeaf = { ...FIXTURE.leaves[1]!, spent: { spend_txid: 'cc'.repeat(32), deduct: 25_000_000 } };
  return {
    ...FIXTURE,
    root: '21'.repeat(32),
    settlement_txid: 'cc'.repeat(32),
    unclaimed: 2,
    leaves: [FIXTURE.leaves[0]!, spentLeaf, FIXTURE.leaves[2]!],
  };
}

/// A fully drained root: every leaf spent, unclaimed 0.
function drainedFixture(): ExitRoot {
  const leaves = FIXTURE.leaves.map((leaf) => ({
    ...leaf,
    spent: { spend_txid: 'cd'.repeat(32), deduct: leaf.amount },
  }));
  return { ...FIXTURE, unclaimed: 0, leaves };
}

describe('mySpkHex (StandardSpk::PubKey script bytes: 0x20 || pk || 0xac)', () => {
  it('wraps the 32-byte x-only pubkey in OP_DATA_32 and OP_CHECKSIG (34 bytes)', () => {
    expect(mySpkHex(MY_PK)).toBe(`20${MY_PK}ac`);
    expect(mySpkHex(MY_PK)).toHaveLength(68);
  });
});

describe('claimableExits (unspent leaves paying my key, across advancing roots)', () => {
  it('keeps my unspent leaves of an all-unspent root, keyed root:index', () => {
    expect(claimableExits([FIXTURE], MY_PK)).toEqual([
      { key: `${FIXTURE.root}:0`, root: FIXTURE, leaf: FIXTURE.leaves[0] },
      { key: `${FIXTURE.root}:1`, root: FIXTURE, leaf: FIXTURE.leaves[1] },
    ]);
  });

  it('keeps my unspent leaves of a continuation-advanced root (a spent sibling does not gate them)', () => {
    const advanced = claimedFixture();
    expect(claimableExits([advanced], MY_PK)).toEqual([
      { key: `${advanced.root}:0`, root: advanced, leaf: advanced.leaves[0] },
    ]);
  });

  it('drops my emptied leaves even when the root still serves them', () => {
    const advanced = claimedFixture();
    expect(claimableExits([advanced], MY_PK).some((t) => t.leaf.spent !== null)).toBe(false);
  });

  it('yields nothing for a drained root (unclaimed 0, every leaf spent)', () => {
    expect(claimableExits([drainedFixture()], MY_PK)).toEqual([]);
  });

  it('drops leaves paying another key', () => {
    const targets = claimableExits([FIXTURE], MY_PK);
    expect(targets.some((t) => t.leaf.spk_hex === mySpkHex(THEIR_PK))).toBe(false);
  });

  it('spans every served root', () => {
    const other: ExitRoot = { ...FIXTURE, root: '31'.repeat(32), leaves: [FIXTURE.leaves[0]!] };
    expect(claimableExits([FIXTURE, other], MY_PK)).toHaveLength(3);
  });
});

describe('claimArgs (full-deduct claim_tx mapping from the served shapes)', () => {
  it('maps every encoder argument from the exit root and leaf', () => {
    const leaf = FIXTURE.leaves[0]!;
    expect(claimArgs(COVENANT, FIXTURE, leaf)).toEqual({
      covenant_id_hex: COVENANT,
      permission_txid_hex: FIXTURE.settlement_txid,
      permission_index: FIXTURE.outpoint_index,
      permission_rent: 50_000_000n,
      old_root_hex: FIXTURE.root,
      old_unclaimed: 3n,
      depth: leaf.siblings.length,
      leaf_index: 0,
      leaf_spk_hex: leaf.spk_hex,
      leaf_amount: 50_000_000n,
      new_root_hex: leaf.full_claim.new_root,
      new_unclaimed: 2n,
      siblings_hex: leaf.siblings,
      fee: 2_000_000n,
    });
  });
});

describe('canAffordClaim (delegate sum, not the single-UTXO rule)', () => {
  const utxo = (amount: bigint): WalletUtxo => ({ txid_hex: 'aa'.repeat(32), index: 0, amount, spk_hex: '00', spk_version: 0 });

  it('affords when the delegate sum covers payout plus fee (split pool)', () => {
    expect(canAffordClaim([utxo(30n), utxo(25n)], 50n, 0n)).toBe(true);
  });

  it('affords at the exact boundary (delegate change may be zero)', () => {
    expect(canAffordClaim([utxo(50n)], 50n, 0n)).toBe(true);
  });

  it('rejects a short or empty pool', () => {
    expect(canAffordClaim([utxo(30n)], 50n, 0n)).toBe(false);
    expect(canAffordClaim([], 50n, 0n)).toBe(false);
  });

  it('counts the fee in the requirement', () => {
    expect(canAffordClaim([utxo(50n)], 50n, 1n)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Submit flow over the real encoder wasm.

/// L1 client whose delegate pool (UTXOs at the deposit address) totals
/// `amounts`; the address argument is ignored like the other mocks.
function delegateClient(amounts: bigint[]) {
  const entries = amounts.map((amount, i) => ({
    outpoint: { transactionId: `d${i}`.repeat(32), index: 0 },
    amount,
    scriptPublicKey: { version: 0, script: '9a'.repeat(10) },
    blockDaaScore: 0n,
    isCoinbase: false,
  }));
  const submitTransaction = vi.fn(
    async (_req: { transaction: Transaction; allowOrphan?: boolean }) => ({ transactionId: 'cafecafe' }),
  );
  const client = {
    getUtxosByAddresses: async () => ({ entries }),
    submitTransaction,
  } as never as RpcClient;
  return { client, submitTransaction };
}

function myClaimRoot(pubkeyHex: string, newUnclaimed: number): { root: ExitRoot; leaf: ExitLeaf } {
  const leaf: ExitLeaf = {
    index: 0,
    spk_hex: mySpkHex(pubkeyHex),
    amount: 50_000_000,
    spent: null,
    // A single-leaf root folds at depth 1: its one sibling is the empty-hash.
    siblings: ['dbc1b4c900ffe48d575b5da5c638040125f65db0fe3e24494b76ea986457d986'],
    full_claim: { new_root: '44'.repeat(32), new_unclaimed: newUnclaimed },
  };
  return { root: { ...FIXTURE, unclaimed: newUnclaimed + 1, leaves: [leaf] }, leaf };
}

describe('submitClaim', () => {
  it('aggregates delegate UTXOs into a full-deduct payout on L1 and witnesses my L1 sum', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { root, leaf } = myClaimRoot(identity.wallet.pubkeyHex, 1);
    const { client, submitTransaction } = delegateClient([30_000_000n, 25_000_000n]);
    const onActivity = vi.fn();

    const txid = await submitClaim({
      identity,
      client,
      covenantId: COVENANT,
      depositAddress: DEPOSIT,
      root,
      leaf,
      balanceBefore: 123n,
      onActivity,
    });

    expect(txid).toBe('cafecafe');
    expect(submitTransaction).toHaveBeenCalledOnce();
    const arg = submitTransaction.mock.calls[0]![0]!;
    expect(arg.allowOrphan).toBe(false);
    expect(arg.transaction).toBeInstanceOf(Transaction);
    // Permission UTXO plus both delegate inputs: claims aggregate, unlike carriers.
    expect(arg.transaction.inputs).toHaveLength(3);
    // Full-leaf deduct: the payout is the whole leaf amount, to my key.
    expect(arg.transaction.outputs[0].value).toBe(50_000_000n);
    expect(arg.transaction.outputs[0].scriptPublicKey.script).toBe(mySpkHex(identity.wallet.pubkeyHex));
    // The permission continuation output carries the served rent.
    expect(arg.transaction.outputs[1].value).toBe(50_000_000n);
    // The delegate change carries the pool remainder minus the burned fee.
    expect(arg.transaction.outputs[2].value).toBe(3_000_000n);
    expect(onActivity).toHaveBeenCalledWith('claim exits', 'cafecafe', { kind: 'l1', before: 123n });
  });

  it('folds the rent into the payout when claiming the last unclaimed leaf', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { root, leaf } = myClaimRoot(identity.wallet.pubkeyHex, 0);
    const { client, submitTransaction } = delegateClient([55_000_000n]);

    await submitClaim({
      identity,
      client,
      covenantId: COVENANT,
      depositAddress: DEPOSIT,
      root,
      leaf,
      balanceBefore: null,
      onActivity: vi.fn(),
    });

    const tx = submitTransaction.mock.calls[0]![0]!.transaction;
    // Payout + folded rent, then the delegate change; no continuation output.
    expect(tx.outputs).toHaveLength(2);
    expect(tx.outputs[0].value).toBe(100_000_000n);
  });

  it('rejects before building when the delegate pool cannot cover the payout', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { root, leaf } = myClaimRoot(identity.wallet.pubkeyHex, 1);
    const { client, submitTransaction } = delegateClient([10_000_000n]);

    await expect(
      submitClaim({
        identity,
        client,
        covenantId: COVENANT,
        depositAddress: DEPOSIT,
        root,
        leaf,
        balanceBefore: null,
        onActivity: vi.fn(),
      }),
    ).rejects.toThrow(/delegate/);
    expect(submitTransaction).not.toHaveBeenCalled();
  });

  it('propagates a rejected submitTx: fee-bearing claims have no /inject fallback', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { root, leaf } = myClaimRoot(identity.wallet.pubkeyHex, 1);
    const { client, submitTransaction } = delegateClient([55_000_000n]);
    submitTransaction.mockRejectedValueOnce(new Error('rejected: missing inputs'));
    const inject = vi.fn();
    vi.stubGlobal('fetch', inject);

    await expect(
      submitClaim({
        identity,
        client,
        covenantId: COVENANT,
        depositAddress: DEPOSIT,
        root,
        leaf,
        balanceBefore: null,
        onActivity: vi.fn(),
      }),
    ).rejects.toThrow(/rejected: missing inputs/);
    expect(inject).not.toHaveBeenCalled();
  });
});
