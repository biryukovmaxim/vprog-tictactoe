//! Wallet pins: schnorr pubkey agreement between the encoder (vprogs locks)
//! and kaspa-wasm (addresses), borsh tx decode, and the submitTx call shape.
//!
//! NOTE: the vendored encoder pkg exposes no JS constructor for `UtxoCandidate`
//! (wasm-bindgen `getter_with_clone` = getters only), so the `*_tx` builders
//! cannot be invoked from JS until that pkg is rebuilt with a constructor.
//! Tx bytes here are hand-encoded from the `kaspa_consensus_core::tx`
//! borsh layout instead; the pubkey pins below do run both real wasm modules.

import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { initKaspa, loadKey, txFromBorsh } from '../wallet';
import initEncoder, { my_ids } from '../wasm/vprog_tictactoe_encoder_wasm.js';
import { PrivateKey, Transaction } from '../kaspa-pkg/kaspa.js';

/// secp256k1 scalar 7 — same fixed key as the encoder crate tests.
const PRIVKEY = '0'.repeat(63) + '7';

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

// --- borsh fixtures: `kaspa_consensus_core::tx::Transaction` layout, LE -----

function hexToBytes(hexStr: string): Uint8Array {
  return Uint8Array.from({ length: hexStr.length / 2 }, (_, i) => parseInt(hexStr.slice(2 * i, 2 * i + 2), 16));
}

class Writer {
  private out: number[] = [];
  u8(v: number) {
    this.out.push(v & 0xff);
  }
  u16(v: number) {
    this.u8(v);
    this.u8(v >>> 8);
  }
  u32(v: number) {
    this.u8(v);
    this.u8(v >>> 8);
    this.u8(v >>> 16);
    this.u8(v >>> 24);
  }
  u64(v: bigint) {
    for (let i = 0n; i < 8n; i++) {
      this.u8(Number((v >> (8n * i)) & 0xffn));
    }
  }
  bytes(b: Uint8Array) {
    this.out.push(...b);
  }
  vec(b: Uint8Array) {
    this.u32(b.length);
    this.bytes(b);
  }
  done(): Uint8Array {
    return Uint8Array.from(this.out);
  }
}

/// raw 0x00..0x1f — non-uniform so display-order reversal is actually tested.
const RAW32 = Uint8Array.from({ length: 32 }, (_, i) => i);
const RAW32_DISPLAY = [...RAW32].reverse().map((b) => b.toString(16).padStart(2, '0')).join('');
const LANE = '33'.repeat(20);
const PAYLOAD = hexToBytes('beefcafe');

function fixtureTxBytes(): Uint8Array {
  const w = new Writer();
  w.u16(1); // version (TX_VERSION_TOCCATA)
  w.u32(1); // 1 input
  w.bytes(RAW32); // outpoint txid (raw order)
  w.u32(3); // outpoint index
  w.vec(hexToBytes('dead')); // signature script
  w.u64(0xffffffffffffffffn); // sequence
  w.u8(1);
  w.u16(7); // compute_commit: ComputeBudget(7)
  w.u32(2); // 2 outputs
  w.u64(123456789n); // value
  w.u16(0); // spk version
  w.vec(hexToBytes(`20${'ab'.repeat(32)}ac`)); // spk script
  w.u8(1); // covenant: Some
  w.u16(0); // authorizing input
  w.bytes(RAW32); // covenant id
  w.u64(1000n); // value
  w.u16(0);
  w.vec(hexToBytes('51')); // OP_TRUE
  w.u8(0); // covenant: None
  w.u64(42n); // lock time
  w.bytes(hexToBytes(LANE)); // subnetwork id (20 bytes)
  w.u64(0n); // gas
  w.vec(PAYLOAD); // payload
  w.u64(9n); // storage mass
  w.bytes(RAW32); // id (raw order)
  return w.done();
}

describe('borsh tx decode', () => {
  it('decodes the consensus Transaction layout and consumes every byte', () => {
    const tx = txFromBorsh(fixtureTxBytes());
    expect(tx.version).toBe(1);
    expect(tx.inputs).toHaveLength(1);
    expect(tx.inputs[0].previousOutpoint.transactionId).toBe(RAW32_DISPLAY);
    expect(tx.inputs[0].previousOutpoint.index).toBe(3);
    expect(tx.inputs[0].signatureScript).toBe('dead');
    expect(tx.inputs[0].sequence).toBe(0xffffffffffffffffn);
    expect(tx.inputs[0].computeBudget).toBe(7);
    expect(tx.outputs).toHaveLength(2);
    expect(tx.outputs[0].value).toBe(123456789n);
    expect(tx.outputs[0].scriptPublicKey.script).toBe(`20${'ab'.repeat(32)}ac`);
    expect(tx.outputs[0].covenant.covenantId).toBe(RAW32_DISPLAY);
    expect(tx.outputs[1].covenant).toBeUndefined();
    expect(tx.lockTime).toBe(42n);
    expect(tx.subnetworkId).toBe(LANE);
    expect(tx.gas).toBe(0n);
    expect(tx.payload).toBe('beefcafe');
    expect(tx.storageMass).toBe(9n);
    expect(tx.id).toBe(RAW32_DISPLAY);

    // The decoded object must be accepted by the wasm Transaction constructor.
    expect(() => new Transaction(tx)).not.toThrow();
  });

  it('rejects trailing bytes', () => {
    const bytes = fixtureTxBytes();
    expect(() => txFromBorsh(new Uint8Array([...bytes, 0]))).toThrow(/trailing/);
  });
});

describe('submitTx', () => {
  it('submits the decoded transaction with allowOrphan false (mock client)', async () => {
    const submitTransaction = vi.fn(
      async (_req: { transaction: Transaction; allowOrphan?: boolean }) => ({ transactionId: 'deadbeef' }),
    );
    const wallet = loadKey(PRIVKEY);
    const txid = await wallet.submitTx({ submitTransaction } as never, fixtureTxBytes());
    expect(txid).toBe('deadbeef');
    expect(submitTransaction).toHaveBeenCalledOnce();
    const arg = submitTransaction.mock.calls[0]![0]!;
    expect(arg.allowOrphan).toBe(false);
    expect(arg.transaction).toBeInstanceOf(Transaction);
  });
});
