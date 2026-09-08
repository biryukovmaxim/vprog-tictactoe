//! Wallet pins: schnorr pubkey agreement between the encoder (vprogs locks)
//! and kaspa-wasm (addresses), borsh tx decode of real encoder output, and
//! the submitTx call shape.

import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { initKaspa, loadKey, txFromBorsh } from '../wallet';
import initEncoder, {
  UtxoCandidate,
  create_game_tx,
  my_ids,
  network_params,
} from '../wasm/vprog_tictactoe_encoder_wasm.js';
import { PrivateKey, Transaction } from '../kaspa-pkg/kaspa.js';

/// secp256k1 scalar 7 — same fixed key as the encoder crate tests.
const PRIVKEY = '0'.repeat(63) + '7';
const LANE = '33'.repeat(20);

beforeAll(async () => {
  const encoderWasm = readFileSync(new URL('../wasm/vprog_tictactoe_encoder_wasm_bg.wasm', import.meta.url));
  const kaspaWasm = readFileSync(new URL('../kaspa-pkg/kaspa_bg.wasm', import.meta.url));
  await Promise.all([initEncoder(encoderWasm), initKaspa(kaspaWasm)]);
});

describe('pubkey agreement between encoder and kaspa-wasm', () => {
  it('my_ids().pubkey_hex equals the kaspa-wasm x-only derivation', () => {
    const ids = my_ids(PRIVKEY);
    const kaspaHex = new PrivateKey(PRIVKEY).toPublicKey().toXOnlyPublicKey().toString();
    // THE pin: vprogs schnorr locks and kaspa addresses must agree on the key.
    expect(ids.pubkey_hex).toBe(kaspaHex);
  });

  it('loadKey derives the address and same pubkey hex', () => {
    const wallet = loadKey(PRIVKEY);
    expect(wallet.pubkeyHex).toBe(my_ids(PRIVKEY).pubkey_hex);
    expect(wallet.address).toMatch(/^kaspasim:/);
  });
});

/// A real, signed, unfunded `CreateGame` carrier built from JS.
function createGameBytes(): Uint8Array {
  const ids = my_ids(PRIVKEY);
  const utxo = new UtxoCandidate('21'.repeat(32), 3, 1_000_000_000n, `20${ids.pubkey_hex}ac`, 0);
  return create_game_tx(
    PRIVKEY,
    network_params('simnet'),
    utxo,
    loadKey(PRIVKEY).address,
    LANE,
    '42'.repeat(32), // config id (parsed, not cross-checked)
    0n, // games started
    50_000_000n, // stake
    3, // rounds
    1, // mark X
    0n, // no deposit: single action, change-only outputs
    '77'.repeat(32), // covenant id (unused without a deposit)
  );
}

describe('borsh tx decode of real encoder output', () => {
  it('decodes a signed JS-built carrier and consumes every byte', () => {
    const tx = txFromBorsh(createGameBytes());
    expect(tx.version).toBe(1);
    expect(tx.inputs).toHaveLength(1);
    expect(tx.inputs[0].previousOutpoint.transactionId).toBe('21'.repeat(32));
    expect(tx.inputs[0].previousOutpoint.index).toBe(3);
    expect(tx.inputs[0].signatureScript.length).toBeGreaterThan(0);
    expect(tx.outputs).toHaveLength(1); // unfunded create: change only
    expect(tx.outputs[0].value).toBeGreaterThan(0n);
    expect(tx.outputs[0].value).toBeLessThan(1_000_000_000n);
    expect(tx.subnetworkId).toBe(LANE);
    expect(tx.payload.length).toBeGreaterThan(0);
    expect(tx.id).toMatch(/^[0-9a-f]{64}$/);

    // The decoded object must be accepted by the wasm Transaction constructor.
    expect(() => new Transaction(tx)).not.toThrow();
  });

  it('rejects trailing bytes', () => {
    const bytes = createGameBytes();
    expect(() => txFromBorsh(new Uint8Array([...bytes, 0]))).toThrow(/trailing/);
  });
});

describe('submitTx', () => {
  it('submits the decoded transaction with allowOrphan false (mock client)', async () => {
    const submitTransaction = vi.fn(
      async (_req: { transaction: Transaction; allowOrphan?: boolean }) => ({ transactionId: 'deadbeef' }),
    );
    const wallet = loadKey(PRIVKEY);
    const txid = await wallet.submitTx({ submitTransaction } as never, createGameBytes());
    expect(txid).toBe('deadbeef');
    expect(submitTransaction).toHaveBeenCalledOnce();
    const arg = submitTransaction.mock.calls[0]![0]!;
    expect(arg.allowOrphan).toBe(false);
    expect(arg.transaction).toBeInstanceOf(Transaction);
  });
});
