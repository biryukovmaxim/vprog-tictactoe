//! Composition pins: entry-deposit sizing, affordability gate, UTXO pick, the
//! protocol config-id constant, and the create/join submit flow over the real
//! encoder wasm (mock L1 client + mock DA account fetch).

import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import {
  CONFIG_ID_HEX,
  FEE_ESTIMATE,
  MIN_CREATE_BALANCE,
  canAfford,
  entryDeposit,
  pickUtxo,
  submitEntry,
} from '../composition';
import { loadIdentity } from '../KeyBar';
import { initKaspa, type WalletUtxo } from '../wallet';
import initEncoder, { my_ids } from 'vprog-tictactoe-encoder-wasm';
import type { RpcClient } from 'kaspa-wasm';
import { Transaction } from 'kaspa-wasm';

/// secp256k1 scalar 7 — same fixed key as the encoder crate tests.
const PRIVKEY = '0'.repeat(63) + '7';
const LANE = '33'.repeat(20);
const COVENANT = '77'.repeat(32);

beforeAll(async () => {
  const encoderWasm = readFileSync(createRequire(import.meta.url).resolve('vprog-tictactoe-encoder-wasm/vprog_tictactoe_encoder_wasm_bg.wasm'));
  const kaspaWasm = readFileSync(createRequire(import.meta.url).resolve('kaspa-wasm/kaspa_bg.wasm'));
  await Promise.all([initEncoder(encoderWasm), initKaspa(kaspaWasm)]);
});

afterEach(() => vi.unstubAllGlobals());

describe('entryDeposit', () => {
  it('existing account short by 300 deposits 300', () => {
    expect(entryDeposit({ exists: true, balance: 700n, stake: 1000n, minCreateBalance: 1000n })).toBe(300n);
  });

  it('no account, stake 500, min-create 1000 deposits 1000 (min covers the stake)', () => {
    expect(entryDeposit({ exists: false, balance: 0n, stake: 500n, minCreateBalance: 1000n })).toBe(1000n);
  });

  it('sufficient balance deposits 0', () => {
    expect(entryDeposit({ exists: true, balance: 1500n, stake: 1000n, minCreateBalance: 1000n })).toBe(0n);
  });

  it('stake equal to balance deposits 0', () => {
    expect(entryDeposit({ exists: true, balance: 1000n, stake: 1000n, minCreateBalance: 1000n })).toBe(0n);
  });

  it('no account with stake above min-create deposits the stake', () => {
    expect(entryDeposit({ exists: false, balance: 0n, stake: 5000n, minCreateBalance: 1000n })).toBe(5000n);
  });
});

describe('canAfford (largest single UTXO, not the sum)', () => {
  const utxo = (amount: bigint): WalletUtxo => ({ txid_hex: 'aa', index: 0, amount, spk_hex: '00', spk_version: 0 });

  it('affords when the largest UTXO strictly covers deposit plus fee', () => {
    expect(canAfford([utxo(300n), utxo(1000n + FEE_ESTIMATE + 1n)], 1000n, FEE_ESTIMATE)).toBe(true);
  });

  it('rejects a split wallet: sum covers but no single UTXO does', () => {
    expect(canAfford([utxo(600n), utxo(600n)], 1000n, FEE_ESTIMATE)).toBe(false);
  });

  it('rejects a single UTXO exactly at deposit plus fee (change must stay positive)', () => {
    expect(canAfford([utxo(1000n + FEE_ESTIMATE)], 1000n, FEE_ESTIMATE)).toBe(false);
  });

  it('rejects an empty UTXO set', () => {
    expect(canAfford([], 1000n, FEE_ESTIMATE)).toBe(false);
  });
});

describe('pickUtxo', () => {
  const utxo = (amount: bigint, txid: string): WalletUtxo => ({
    txid_hex: txid,
    index: 0,
    amount,
    spk_hex: '00',
    spk_version: 0,
  });

  it('picks the largest UTXO strictly covering the need', () => {
    const got = pickUtxo([utxo(300n, 'aa'), utxo(1200n, 'bb'), utxo(200n, 'cc')], 500n);
    expect(got?.txid_hex).toBe('bb');
  });

  it('returns null when no single UTXO strictly covers (change must stay positive)', () => {
    expect(pickUtxo([utxo(500n, 'aa')], 500n)).toBeNull();
  });

  it('returns null on an empty UTXO set', () => {
    expect(pickUtxo([], 1n)).toBeNull();
  });
});

describe('protocol constants', () => {
  it('CONFIG_ID_HEX is sha256 over the Config domain tag (guest resources/id.rs)', () => {
    expect(CONFIG_ID_HEX).toBe(createHash('sha256').update(Uint8Array.of(4)).digest('hex'));
  });

  it('domain-prepend model agrees with my_ids user derivation', () => {
    // Justifies the constant above: resource ids are sha256(domain || seed).
    const ids = my_ids(PRIVKEY);
    const want = createHash('sha256')
      .update(Uint8Array.of(6))
      .update(Buffer.from(ids.lock_hash_hex, 'hex'))
      .digest('hex');
    expect(want).toBe(ids.user_id_hex);
  });

  it('MIN_CREATE_BALANCE mirrors the guest policy constant (1000 sompi)', () => {
    expect(MIN_CREATE_BALANCE).toBe(1000n);
  });
});

// ---------------------------------------------------------------------------
// submitEntry over the real encoder wasm.

function stubAccount(userIdHex: string, body: unknown) {
  const f = vi.fn(async (input: RequestInfo | URL) => {
    expect(String(input)).toBe(`/api/accounts/${userIdHex}`);
    return { ok: true, json: async () => body } as Response;
  });
  vi.stubGlobal('fetch', f);
  return f;
}

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
    submitTransaction,
  } as never as RpcClient;
  return { client, submitTransaction };
}

describe('submitEntry (create with auto-deposit for a new user)', () => {
  it('builds a deposit+create carrier, submits it, and reports the deposit', async () => {
    const identity = await loadIdentity(PRIVKEY);
    stubAccount(identity.userIdHex, { exists: false });
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, [1_000_000_000n]);
    const onActivity = vi.fn();

    const res = await submitEntry({
      identity,
      client,
      lane: LANE,
      covenantId: COVENANT,
      entry: { kind: 'create', stake: 50_000_000n, rounds: 3, mark: 1 },
      onActivity,
    });

    expect(res.txid).toBe('cafecafe');
    // New user: the deposit covers the stake itself (max(stake, min-create)).
    expect(res.deposit).toBe(50_000_000n);
    expect(submitTransaction).toHaveBeenCalledOnce();
    const arg = submitTransaction.mock.calls[0]![0]!;
    expect(arg.allowOrphan).toBe(false);
    expect(arg.transaction).toBeInstanceOf(Transaction);
    expect(arg.transaction.outputs).toHaveLength(2); // covenant deposit at 0 + change
    expect(arg.transaction.outputs[0].value).toBe(50_000_000n);
    expect(onActivity).toHaveBeenCalledTimes(1);
    expect(onActivity).toHaveBeenCalledWith('create game', 'cafecafe');
  });

  it('rejects with a funding hint and signals the needed L1 amount when no UTXO covers the deposit', async () => {
    const identity = await loadIdentity(PRIVKEY);
    stubAccount(identity.userIdHex, { exists: false });
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, []);
    const onNeedsFunding = vi.fn();

    await expect(
      submitEntry({
        identity,
        client,
        lane: LANE,
        covenantId: COVENANT,
        entry: { kind: 'create', stake: 50_000_000n, rounds: 3, mark: 1 },
        onActivity: vi.fn(),
        onNeedsFunding,
      }),
    ).rejects.toThrow(/fund kaspasim:/);
    expect(submitTransaction).not.toHaveBeenCalled();
    expect(onNeedsFunding).toHaveBeenCalledWith(50_000_000n + FEE_ESTIMATE);
  });
});

describe('submitEntry (join without deposit for a funded account)', () => {
  it('builds a change-only join carrier when the L2 balance covers the stake', async () => {
    const identity = await loadIdentity(PRIVKEY);
    stubAccount(identity.userIdHex, { exists: true, balance: 300_000_000, games_started: 5 });
    const { client, submitTransaction } = mockClient(identity.wallet.pubkeyHex, [1_000_000_000n]);
    const onActivity = vi.fn();

    const res = await submitEntry({
      identity,
      client,
      lane: LANE,
      covenantId: COVENANT,
      entry: { kind: 'join', stake: 50_000_000n, gameId: '55'.repeat(32) },
      onActivity,
    });

    expect(res.deposit).toBe(0n);
    const arg = submitTransaction.mock.calls[0]![0]!;
    expect(arg.transaction.outputs).toHaveLength(1); // change only, no covenant output
    expect(arg.transaction.outputs[0].value).toBeGreaterThan(0n);
    expect(onActivity).toHaveBeenCalledTimes(1);
    expect(onActivity).toHaveBeenCalledWith('join game', 'cafecafe');
  });
});
