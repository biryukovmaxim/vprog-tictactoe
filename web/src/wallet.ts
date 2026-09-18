//! Wallet plumbing over kaspa-wasm: key loading, L1 UTXOs, tx submit.
//!
//! kaspa-wasm is the locally built `wasm32-core` package, vendored as the
//! `file:vendor/kaspa-wasm-2.0.1.tgz` dep (rusty-kaspa @ v2.0.1; npm only
//! ships 0.13.x).
//! The wasm module exposes NO borsh constructor, so the tx-bytes decoder below
//! mirrors `kaspa_consensus_core::tx::Transaction` borsh (declaration order)
//! and feeds the plain-object `Transaction` constructor — the module's actual
//! equivalent of the missing `Transaction.deserialize(bytes)`.

import initKaspaModule, {
  NetworkType,
  PrivateKey,
  RpcClient,
  Transaction,
  type Keypair,
} from 'kaspa-wasm';

/// wRPC endpoint of the L1 node (TT_WRPC_URL equivalent).
export const WRPC_URL: string = (import.meta.env.VITE_WRPC_URL as string) ?? 'ws://127.0.0.1:17210';
/// Network the demo runs on; also the address prefix selector.
export const NETWORK: string = (import.meta.env.VITE_NETWORK as string) ?? 'simnet';

let kaspaInit: Promise<unknown> | null = null;

/// Loads the kaspa-wasm module once. `input` (wasm bytes) is required under
/// Node (vitest); in the browser the default export resolves the URL itself.
export function initKaspa(input?: BufferSource | Promise<BufferSource>): Promise<unknown> {
  kaspaInit ??= initKaspaModule(input);
  return kaspaInit;
}

/// One spendable L1 UTXO, shaped as the encoder-wasm `UtxoCandidate`.
export interface WalletUtxo {
  txid_hex: string;
  index: number;
  amount: bigint;
  spk_hex: string;
  spk_version: number;
}

/// A loaded identity: kaspa-wasm keypair plus derived rollup identity.
export interface Wallet {
  keypair: Keypair;
  /// Kaspa bech32 address (schnorr P2PK) that must receive L1 funds.
  address: string;
  /// X-only (BIP340/schnorr) public key hex — the rollup identity.
  pubkeyHex: string;
  /// Live UTXOs of `address`; feeds the encoder tx builders.
  l1Utxos(client: RpcClient): Promise<WalletUtxo[]>;
  /// Decodes borsh tx bytes and submits them; resolves to the txid hex.
  submitTx(client: RpcClient, bytes: Uint8Array): Promise<string>;
}

function networkType(name: string): NetworkType {
  switch (name) {
    case 'mainnet':
      return NetworkType.Mainnet;
    case 'devnet':
      return NetworkType.Devnet;
    case 'simnet':
      return NetworkType.Simnet;
    default:
      return NetworkType.Testnet;
  }
}

/// Derives the wallet for one private key hex. `initKaspa` must have settled.
export function loadKey(privkeyHex: string): Wallet {
  const priv = new PrivateKey(privkeyHex);
  const keypair = priv.toKeypair();
  const address = keypair.toAddress(networkType(NETWORK)).toString();
  // `toPublicKey().toXOnlyPublicKey()` is the schnorr identity the encoder
  // binds locks to (`my_ids().pubkey_hex` agrees — pinned by wallet.test.ts).
  const pubkeyHex = priv.toPublicKey().toXOnlyPublicKey().toString();

  return {
    keypair,
    address,
    pubkeyHex,
    async l1Utxos(client: RpcClient): Promise<WalletUtxo[]> {
      // Spendable only: coinbase outputs need 100 daa-score maturity, and this
      // wallet class is typically a miner payout address with fresh rewards.
      const { virtualDaaScore } = await client.getBlockDagInfo();
      const { entries } = await client.getUtxosByAddresses({ addresses: [address] });
      return entries
        .filter((e) => !e.isCoinbase || BigInt(e.blockDaaScore) + 100n <= BigInt(virtualDaaScore))
        .map((e) => ({
          txid_hex: e.outpoint.transactionId,
          index: e.outpoint.index,
          amount: e.amount,
          spk_hex: e.scriptPublicKey.script,
          spk_version: e.scriptPublicKey.version,
        }));
    },
    submitTx: (client: RpcClient, bytes: Uint8Array) => submitTx(client, bytes),
  };
}

/// Connects an RpcClient to `WRPC_URL` on `NETWORK`.
export async function connectClient(): Promise<RpcClient> {
  await initKaspa();
  const client = new RpcClient({ url: WRPC_URL, networkId: NETWORK });
  await client.connect();
  return client;
}

async function submitTx(client: RpcClient, bytes: Uint8Array): Promise<string> {
  const tx = new Transaction(txFromBorsh(bytes));
  const { transactionId } = await client.submitTransaction({ transaction: tx, allowOrphan: false });
  return transactionId;
}

// ---------------------------------------------------------------------------
// Borsh decoder for `kaspa_consensus_core::tx::Transaction` (Toccata, v1).
// Layout (declaration order, all integers LE):
//   version u16
//   inputs  Vec[{ outpoint{txid [u8;32] raw, index u32}, sig_script Vec<u8>,
//                 sequence u64, compute_commit enum{0: SigopCount u8, 1: ComputeBudget u16} }]
//   outputs Vec[{ value u64, spk{version u16, script Vec<u8>},
//                 covenant Option{0, 1: {authorizing_input u16, covenant_id [u8;32]}} }]
//   lock_time u64, subnetwork_id [u8;20], gas u64, payload Vec<u8>,
//   storage_mass u64, id [u8;32]
// Hash cross the wasm boundary as plain raw-order hex: this kaspa-wasm build
// has symmetric plain Display/FromStr, so reversing here would flip the
// outpoint txid and the node would reject the spend as an orphan.

class Reader {
  pos = 0;
  constructor(private readonly b: Uint8Array) {}
  u8(): number {
    return this.b[this.pos++];
  }
  u16(): number {
    const v = (this.b[this.pos]! | (this.b[this.pos + 1]! << 8)) >>> 0;
    this.pos += 2;
    return v;
  }
  u32(): number {
    const v = (this.b[this.pos]! | (this.b[this.pos + 1]! << 8) | (this.b[this.pos + 2]! << 16) | (this.b[this.pos + 3]! << 24)) >>> 0;
    this.pos += 4;
    return v;
  }
  u64(): bigint {
    return BigInt(this.u32()) + 0x1_0000_0000n * BigInt(this.u32());
  }
  bytes(n: number): Uint8Array {
    const v = this.b.subarray(this.pos, this.pos + n);
    if (v.length !== n) throw new Error('borsh: unexpected end of transaction bytes');
    this.pos += n;
    return v;
  }
  vec(): Uint8Array {
    return this.bytes(this.u32());
  }
}

const HEX: string[] = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, '0'));

function hex(bytes: Uint8Array): string {
  let s = '';
  for (const b of bytes) s += HEX[b];
  return s;
}

/// kaspa hash hex: plain raw-order bytes (see the decoder header note).
function hashHex(raw: Uint8Array): string {
  return hex(raw);
}

/// Decode borsh tx bytes into the plain `ITransaction` object the wasm
/// `Transaction` constructor accepts.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function txFromBorsh(bytes: Uint8Array): any {
  const r = new Reader(bytes);
  const version = r.u16();

  const inputs = Array.from({ length: r.u32() }, () => {
    const transactionId = hashHex(r.bytes(32));
    const index = r.u32();
    const signatureScript = hex(r.vec());
    const sequence = r.u64();
    const input: {
      previousOutpoint: { transactionId: string; index: number };
      signatureScript: string;
      sequence: bigint;
      sigOpCount: number;
      computeBudget?: number;
    } = { previousOutpoint: { transactionId, index }, signatureScript, sequence, sigOpCount: 0 };
    // ComputeCommit: 0 = SigopCount(u8), 1 = ComputeBudget(u16).
    if (r.u8() === 0) input.sigOpCount = r.u8();
    else input.computeBudget = r.u16();
    return input;
  });

  const outputs = Array.from({ length: r.u32() }, () => {
    const value = r.u64();
    const scriptPublicKey = { version: r.u16(), script: hex(r.vec()) };
    let covenant;
    if (r.u8() === 1) covenant = { authorizingInput: r.u16(), covenantId: hashHex(r.bytes(32)) };
    return { value, scriptPublicKey, covenant };
  });

  const tx = {
    version,
    inputs,
    outputs,
    lockTime: r.u64(),
    subnetworkId: hex(r.bytes(20)),
    gas: r.u64(),
    payload: hex(r.vec()),
    storageMass: r.u64(),
    id: hashHex(r.bytes(32)),
  };
  if (r.pos !== bytes.length) throw new Error('borsh: trailing bytes after transaction');
  return tx;
}
