//! Transfer/withdraw pins: dest-lock derivation and the create-vs-plain
//! branch (pure), the withdrawal floor, and the submit flows over the real
//! encoder wasm (mock L1 client).

import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { FEE_ESTIMATE, submitTransfer, submitWithdraw } from '../composition';
import { loadIdentity } from '../KeyBar';
import { destLockHash, destUserResource, meetsWithdrawFloor, parseSompi, transferArgs } from '../transfer';
import { initKaspa } from '../wallet';
import initEncoder, { my_ids } from 'vprog-tictactoe-encoder-wasm';
import type { RpcClient } from 'kaspa-wasm';
import { Transaction } from 'kaspa-wasm';

/// secp256k1 scalar 7 — same fixed key as the encoder crate tests.
const PRIVKEY = '0'.repeat(63) + '7';
const LANE = '33'.repeat(20);
const DEST = '55'.repeat(32);

beforeAll(async () => {
  const encoderWasm = readFileSync(createRequire(import.meta.url).resolve('vprog-tictactoe-encoder-wasm/vprog_tictactoe_encoder_wasm_bg.wasm'));
  const kaspaWasm = readFileSync(createRequire(import.meta.url).resolve('kaspa-wasm/kaspa_bg.wasm'));
  await Promise.all([initEncoder(encoderWasm), initKaspa(kaspaWasm)]);
});

afterEach(() => vi.unstubAllGlobals());

describe('destLockHash (schnorr lock id_hash: sha256 over 0x03 || 0x01 || pubkey)', () => {
  it('matches the domain/tag/body hash for a fixed pubkey', async () => {
    const pubkey = '55'.repeat(32);
    const want = createHash('sha256').update(Buffer.of(3, 1, ...Buffer.from(pubkey, 'hex'))).digest('hex');
    expect(await destLockHash(pubkey)).toBe(want);
  });

  it("agrees with the encoder's lock hash for my own key (real wasm pin)", async () => {
    const identity = await loadIdentity(PRIVKEY);
    expect(await destLockHash(identity.wallet.pubkeyHex)).toBe(my_ids(PRIVKEY).lock_hash_hex);
  });

  it('changes with the pubkey', async () => {
    expect(await destLockHash('01'.repeat(32))).not.toBe(await destLockHash('02'.repeat(32)));
  });
});

describe('destUserResource (user id over the dest lock: sha256(0x06 || lock))', () => {
  it("chains to the encoder's user id for my own key (real wasm pin)", async () => {
    const identity = await loadIdentity(PRIVKEY);
    expect(await destUserResource(identity.wallet.pubkeyHex)).toBe(my_ids(PRIVKEY).user_id_hex);
  });
});

describe('transferArgs (create-vs-plain branch)', () => {
  it('plain: existing destination drops any supplied pubkey', () => {
    expect(transferArgs({ exists: true, pubkeyHex: '66'.repeat(32) })).toEqual({ dest_exists: true });
  });

  it('create: missing destination carries the dest pubkey', () => {
    expect(transferArgs({ exists: false, pubkeyHex: '66'.repeat(32) })).toEqual({
      dest_exists: false,
      dest_pubkey_hex: '66'.repeat(32),
    });
  });

  it('create without a pubkey is rejected', () => {
    expect(() => transferArgs({ exists: false })).toThrow(/pubkey/);
  });
});

describe('meetsWithdrawFloor', () => {
  it('no configured floor accepts any positive amount', () => {
    expect(meetsWithdrawFloor(1n, undefined)).toBe(true);
  });

  it('enforces the configured minimum, boundary inclusive', () => {
    expect(meetsWithdrawFloor(999n, 1000)).toBe(false);
    expect(meetsWithdrawFloor(1000n, 1000)).toBe(true);
    expect(meetsWithdrawFloor(1001n, 1000)).toBe(true);
  });
});

describe('parseSompi', () => {
  it('0.5 KAS parses to sompi', () => {
    expect(parseSompi('0.5')).toBe(50_000_000n);
  });

  it('rejects zero, negative, and non-numeric input', () => {
    expect(parseSompi('0')).toBeNull();
    expect(parseSompi('-1')).toBeNull();
    expect(parseSompi('abc')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Submit flows over the real encoder wasm.

function mockClient(pubkeyHex: string, amounts: bigint[]) {
  const entries = amounts.map((amount, i) => ({
    outpoint: { transactionId: `${String(i).padStart(2, '0')}21`.repeat(16), index: 0 },
    amount,
    scriptPublicKey: { version: 0, script: `20${pubkeyHex}ac` },
    blockDaaScore: 0n,
    isCoinbase: false,
  }));
  const submitTransaction = vi.fn(
    async (_req: { transaction: Transaction; allowOrphan?: boolean }) => ({ transactionId: 'cafecafe' }),
  );
  const client = {
    getUtxosByAddresses: async () => ({ entries }),
    getBlockDagInfo: async () => ({ virtualDaaScore: 10n ** 12n }),
    submitTransaction,
  } as never as RpcClient;
  return { client, submitTransaction };
}

function balanceWitness(before: bigint | null) {
  return { kind: 'balance' as const, before };
}

describe('submitTransfer', () => {
  it('plain transfer builds a change-only carrier and reports a balance witness', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, [1_000_000_000n]);
    const onActivity = vi.fn();

    const txid = await submitTransfer({
      identity,
      client,
      lane: LANE,
      destUserIdHex: DEST,
      destExists: true,
      amount: 50_000_000n,
      balanceBefore: 123n,
      onActivity,
    });

    expect(txid).toBe('cafecafe');
    expect(submitTransaction).toHaveBeenCalledOnce();
    const arg = submitTransaction.mock.calls[0]![0]!;
    expect(arg.allowOrphan).toBe(false);
    expect(arg.transaction).toBeInstanceOf(Transaction);
    expect(arg.transaction.outputs).toHaveLength(1); // change only, no deposit
    expect(arg.transaction.outputs[0].value).toBeGreaterThan(0n);
    expect(onActivity).toHaveBeenCalledWith('transfer', 'cafecafe', balanceWitness(123n));
  });

  it('creating transfer submits when the dest pubkey is supplied', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, [1_000_000_000n]);

    const txid = await submitTransfer({
      identity,
      client,
      lane: LANE,
      destUserIdHex: DEST,
      destExists: false,
      destPubkeyHex: '66'.repeat(32),
      amount: 50_000_000n,
      balanceBefore: null,
      onActivity: vi.fn(),
    });

    expect(txid).toBe('cafecafe');
    expect(submitTransaction).toHaveBeenCalledOnce();
  });

  it('creating transfer without a pubkey rejects before building', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, [1_000_000_000n]);

    await expect(
      submitTransfer({
        identity,
        client,
        lane: LANE,
        destUserIdHex: DEST,
        destExists: false,
        amount: 50_000_000n,
        balanceBefore: null,
        onActivity: vi.fn(),
      }),
    ).rejects.toThrow(/pubkey/);
    expect(submitTransaction).not.toHaveBeenCalled();
  });

  it('signals funding when no UTXO covers the fee pad', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, []);
    const onNeedsFunding = vi.fn();

    await expect(
      submitTransfer({
        identity,
        client,
        lane: LANE,
        destUserIdHex: DEST,
        destExists: true,
        amount: 50_000_000n,
        balanceBefore: null,
        onActivity: vi.fn(),
        onNeedsFunding,
      }),
    ).rejects.toThrow(/fund kaspasim:/);
    expect(submitTransaction).not.toHaveBeenCalled();
    expect(onNeedsFunding).toHaveBeenCalledWith(FEE_ESTIMATE);
  });
});

describe('submitWithdraw', () => {
  it('builds a change-only carrier to own key and reports a balance witness', async () => {
    const identity = await loadIdentity(PRIVKEY);
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, [1_000_000_000n]);
    const onActivity = vi.fn();

    const txid = await submitWithdraw({
      identity,
      client,
      lane: LANE,
      amount: 50_000_000n,
      balanceBefore: 999n,
      onActivity,
    });

    expect(txid).toBe('cafecafe');
    expect(submitTransaction).toHaveBeenCalledOnce();
    const arg = submitTransaction.mock.calls[0]![0]!;
    expect(arg.transaction.outputs).toHaveLength(1); // change only; the exit pays out at settlement
    expect(onActivity).toHaveBeenCalledWith('withdraw', 'cafecafe', balanceWitness(999n));
  });
});
