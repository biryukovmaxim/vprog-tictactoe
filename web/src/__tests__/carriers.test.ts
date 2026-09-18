//! Carrier-decoder pins: every demo carrier shape built by the real encoder
//! wasm must decode to its named actions (payload layout mirror in carriers.ts).

import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { beforeAll, describe, expect, it } from 'vitest';
import { CONFIG_ID_HEX } from '../composition';
import { decodeLanePayload } from '../carriers';
import { txFromBorsh } from '../wallet';
import initEncoder, {
  create_game_tx,
  join_game_tx,
  my_ids,
  network_params,
  transfer_tx,
  turn_tx,
  withdraw_tx,
  UtxoCandidate,
} from 'vprog-tictactoe-encoder-wasm';
import { initKaspa, loadKey } from '../wallet';

/// secp256k1 scalar 7 — same fixed key as the encoder crate tests.
const PRIVKEY = '0'.repeat(63) + '7';
const LANE = '33'.repeat(20);
const COVENANT = '77'.repeat(32);
const GAME = '11'.repeat(32);

let ids: ReturnType<typeof my_ids>;
let wallet: ReturnType<typeof loadKey>;
let net: ReturnType<typeof network_params>;

beforeAll(async () => {
  const encoderWasm = readFileSync(createRequire(import.meta.url).resolve('vprog-tictactoe-encoder-wasm/vprog_tictactoe_encoder_wasm_bg.wasm'));
  await initEncoder(encoderWasm);
  const kaspaWasm = readFileSync(createRequire(import.meta.url).resolve('kaspa-wasm/kaspa_bg.wasm'));
  await initKaspa(kaspaWasm);
  ids = my_ids(PRIVKEY);
  wallet = loadKey(PRIVKEY);
  net = network_params('simnet');
});

const utxo = () => new UtxoCandidate('ab'.repeat(32), 0, 1_000_000_000n, 'cd'.repeat(35), 0);
const payloadOf = (bytes: Uint8Array): string => txFromBorsh(bytes).payload as string;

describe('decodeLanePayload (real encoder output)', () => {
  it('decodes a single-turn carrier with game and mover resolved through the access list', () => {
    const bytes = turn_tx(PRIVKEY, net, utxo(), wallet.address, LANE, GAME, ids.user_id_hex, 'ee'.repeat(32), 4);
    expect(decodeLanePayload(payloadOf(bytes))).toEqual([
      { kind: 'turn', gameId: GAME, userId: ids.user_id_hex, cell: 4 },
    ]);
  });

  it('decodes the newborn combined deposit+create carrier (skips the deposit lock)', () => {
    const bytes = create_game_tx(
      PRIVKEY, net, utxo(), wallet.address, LANE, CONFIG_ID_HEX, 0n, 50_000_000n, 3, 1, 50_000_000n, COVENANT,
    );
    const actions = decodeLanePayload(payloadOf(bytes));
    expect(actions).toHaveLength(2);
    expect(actions[0]).toEqual({ kind: 'deposit', userId: ids.user_id_hex });
    expect(actions[1]!.kind).toBe('create');
    if (actions[1]!.kind === 'create') {
      expect(actions[1].creatorId).toBe(ids.user_id_hex);
      expect(actions[1].stake).toBe(50_000_000n);
      expect(actions[1].rounds).toBe(3);
      expect(actions[1].mark).toBe(1);
      expect(actions[1].gameId).toMatch(/^[0-9a-f]{64}$/);
    }
  });

  it('decodes deposit+join, transfer-create, and withdraw carriers', () => {
    const join = join_game_tx(PRIVKEY, net, utxo(), wallet.address, LANE, CONFIG_ID_HEX, GAME, 10_000_000n, COVENANT);
    expect(decodeLanePayload(payloadOf(join))).toEqual([
      { kind: 'deposit', userId: ids.user_id_hex },
      { kind: 'join', gameId: GAME, joinerId: ids.user_id_hex },
    ]);

    const transfer = transfer_tx(PRIVKEY, net, utxo(), wallet.address, LANE, 'ff'.repeat(32), false, 5_000n, ids.pubkey_hex);
    expect(decodeLanePayload(payloadOf(transfer))).toEqual([
      { kind: 'transfer', sourceId: ids.user_id_hex, destId: 'ff'.repeat(32), amount: 5_000n, createsDest: true },
    ]);

    const withdraw = withdraw_tx(PRIVKEY, net, utxo(), wallet.address, LANE, CONFIG_ID_HEX, 2_000_000n);
    expect(decodeLanePayload(payloadOf(withdraw))).toEqual([
      { kind: 'withdraw', userId: ids.user_id_hex, amount: 2_000_000n },
    ]);
  });

  it('throws on garbage instead of guessing; an empty payload is no actions', () => {
    expect(() => decodeLanePayload('deadbeef')).toThrow();
    expect(decodeLanePayload('')).toEqual([]);
  });
});
