//! Entry composition for create/join: deposit sizing, affordability, UTXO
//! picking, and the submit flow over the encoder builders. One payload
//! `[Deposit?] + CreateGame|JoinGame` per carrier; the deposit output layout
//! lives entirely inside the encoder — the web only passes `deposit_amount`.

import { useEffect, useState } from 'react';
import { fetchAccount, type ExitLeaf, type ExitRoot } from './da';
import type { Identity } from './KeyBar';
import type { ActivityWitness } from './match';
import { NETWORK, connectClient, type WalletUtxo } from './wallet';
import type { RpcClient } from 'kaspa-wasm';
import { UtxoCandidate, claim_tx, create_game_tx, join_game_tx, network_params, transfer_tx, withdraw_tx } from 'vprog-tictactoe-encoder-wasm';
import { canAffordClaim, claimArgs } from './claim';
import { transferArgs } from './transfer';

/// Minimum sompi a newborn account must be born with; mirrors the guest
/// `MIN_CREATE_BALANCE` (guest/src/program/deposit_policy.rs).
export const MIN_CREATE_BALANCE = 1000n;

/// Conservative carrier fee pad (0.0001 KAS): the real fee is input minus
/// outputs, decided inside the encoder; this only gates the UI affordably.
export const FEE_ESTIMATE = 10_000n;

/// The singleton config resource id: sha256 over the Config domain tag 0x04
/// (guest `config_resource_id`; resource ids are sha256(domain || seed)).
/// Pinned against `my_ids` in composition.test.ts.
export const CONFIG_ID_HEX = 'e52d9c508c502347344d8c07ad91cbd6068afc75ff6292f062a09ca381c89e71';

const POLL_MS = 2_000;

/// Sompi the entry must deposit so the account can pay `stake` afterwards:
/// the stake shortfall, floored by the min-create balance for newborns.
export function entryDeposit(params: {
  exists: boolean;
  balance: bigint;
  stake: bigint;
  minCreateBalance: bigint;
}): bigint {
  const shortfall = params.balance >= params.stake ? 0n : params.stake - params.balance;
  const floor = params.exists ? 0n : params.minCreateBalance;
  return shortfall > floor ? shortfall : floor;
}

/// Whether the wallet can pay `deposit` plus a fee pad: the carrier spends
/// exactly one UTXO, so only a single UTXO strictly covering it affords —
/// a split wallet whose sum covers still cannot fund the carrier.
export function canAfford(utxos: WalletUtxo[], deposit: bigint, feeEstimate: bigint): boolean {
  return pickUtxo(utxos, deposit + feeEstimate) !== null;
}

/// The single UTXO the carrier will spend: the largest one strictly covering
/// `needed` (the encoder leaves the remainder as change, which must be > 0).
export function pickUtxo(utxos: WalletUtxo[], needed: bigint): WalletUtxo | null {
  let best: WalletUtxo | null = null;
  for (const u of utxos) if (u.amount > needed && (!best || u.amount > best.amount)) best = u;
  return best;
}

/// Covenant id of the live deployment: served by /api/config once
/// initialized, always by /api/state.
export function covenantIdOf(configCovenant?: string, stateCovenant?: string | null): string | null {
  return configCovenant ?? stateCovenant ?? null;
}

/// One create or join entry. `stake` sizes the deposit for both kinds; the
/// create-only fields submit the game parameters.
export interface EntryFlow {
  kind: 'create' | 'join';
  stake: bigint;
  /// Create only: round count (1..=255) and mark 1 = X, 2 = O.
  rounds?: number;
  mark?: number;
  /// Join only: the game resource id hex.
  gameId?: string;
}

/// Result of a submitted entry: the L1 carrier txid and the deposit it carried.
export interface EntryReceipt {
  txid: string;
  deposit: bigint;
}

/// Fetches the account fresh, sizes the auto-entry deposit, funds and submits
/// one signed carrier, and reports the row for the activity log.
export async function submitEntry(opts: {
  identity: Identity;
  client: RpcClient;
  /// Lane subnet hex from /api/state.
  lane: string;
  /// Covenant id hex from /api/config (or /api/state).
  covenantId: string;
  entry: EntryFlow;
  onActivity: (label: string, txid: string) => void;
  /// Fired when no single L1 UTXO covers the deposit; feeds the KeyBar
  /// "fund me" hint with the amount that was needed.
  onNeedsFunding?: (needed: bigint) => void;
}): Promise<EntryReceipt> {
  const { identity, client, lane, covenantId, entry, onActivity } = opts;
  const account = await fetchAccount(identity.userIdHex);
  const balance = account.exists && account.balance !== undefined ? BigInt(account.balance) : 0n;
  const deposit = entryDeposit({
    exists: account.exists,
    balance,
    stake: entry.stake,
    minCreateBalance: MIN_CREATE_BALANCE,
  });

  const utxos = await identity.wallet.l1Utxos(client);
  const picked = pickUtxo(utxos, deposit + FEE_ESTIMATE);
  if (!picked) {
    opts.onNeedsFunding?.(deposit + FEE_ESTIMATE);
    throw new Error(`insufficient L1 funds — fund ${identity.wallet.address}`);
  }
  const utxo = new UtxoCandidate(picked.txid_hex, picked.index, picked.amount, picked.spk_hex, picked.spk_version);

  const net = network_params(NETWORK);
  const bytes =
    entry.kind === 'create'
      ? create_game_tx(
          identity.privkeyHex,
          net,
          utxo,
          identity.wallet.address,
          lane,
          CONFIG_ID_HEX,
          BigInt(account.games_started ?? 0),
          entry.stake,
          entry.rounds ?? 3,
          entry.mark ?? 1,
          deposit,
          covenantId,
        )
      : join_game_tx(identity.privkeyHex, net, utxo, identity.wallet.address, lane, CONFIG_ID_HEX, entry.gameId!, deposit, covenantId);

  const txid = await identity.wallet.submitTx(client, bytes);
  onActivity(`${entry.kind} game`, txid);
  return { txid, deposit };
}

/// One L2 transfer carrier: no deposit, change-only — the single-UTXO
/// affordance gate still applies because the carrier spends one UTXO for the
/// fee. Rows ack on my polled L2 balance moving.
export async function submitTransfer(opts: {
  identity: Identity;
  client: RpcClient;
  /// Lane subnet hex from /api/state.
  lane: string;
  /// Dest user-resource id hex.
  destUserIdHex: string;
  /// From `/api/accounts/:destId` — picks the create-vs-plain branch.
  destExists: boolean;
  /// Required when the dest account does not exist (binds its newborn lock).
  destPubkeyHex?: string;
  amount: bigint;
  /// My L2 balance at submit; seeds the ack witness.
  balanceBefore: bigint | null;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
  onNeedsFunding?: (needed: bigint) => void;
}): Promise<string> {
  const { identity, client, lane, destUserIdHex, amount, onActivity } = opts;
  const args = transferArgs({ exists: opts.destExists, pubkeyHex: opts.destPubkeyHex });

  const utxos = await identity.wallet.l1Utxos(client);
  const picked = pickUtxo(utxos, FEE_ESTIMATE);
  if (!picked) {
    opts.onNeedsFunding?.(FEE_ESTIMATE);
    throw new Error(`insufficient L1 funds — fund ${identity.wallet.address}`);
  }
  const utxo = new UtxoCandidate(picked.txid_hex, picked.index, picked.amount, picked.spk_hex, picked.spk_version);

  const bytes = transfer_tx(
    identity.privkeyHex,
    network_params(NETWORK),
    utxo,
    identity.wallet.address,
    lane,
    destUserIdHex,
    args.dest_exists,
    amount,
    args.dest_pubkey_hex ?? null,
  );
  const txid = await identity.wallet.submitTx(client, bytes);
  onActivity('transfer', txid, { kind: 'balance', before: opts.balanceBefore });
  return txid;
}

/// One L2 withdraw carrier: the exit destination is fixed to the caller's own
/// schnorr pubkey inside the encoder; change-only, balance-witnessed like the
/// transfer.
export async function submitWithdraw(opts: {
  identity: Identity;
  client: RpcClient;
  /// Lane subnet hex from /api/state.
  lane: string;
  amount: bigint;
  /// My L2 balance at submit; seeds the ack witness.
  balanceBefore: bigint | null;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
  onNeedsFunding?: (needed: bigint) => void;
}): Promise<string> {
  const { identity, client, lane, amount, onActivity } = opts;

  const utxos = await identity.wallet.l1Utxos(client);
  const picked = pickUtxo(utxos, FEE_ESTIMATE);
  if (!picked) {
    opts.onNeedsFunding?.(FEE_ESTIMATE);
    throw new Error(`insufficient L1 funds — fund ${identity.wallet.address}`);
  }
  const utxo = new UtxoCandidate(picked.txid_hex, picked.index, picked.amount, picked.spk_hex, picked.spk_version);

  const bytes = withdraw_tx(identity.privkeyHex, network_params(NETWORK), utxo, identity.wallet.address, lane, CONFIG_ID_HEX, amount);
  const txid = await identity.wallet.submitTx(client, bytes);
  onActivity('withdraw', txid, { kind: 'balance', before: opts.balanceBefore });
  return txid;
}

/// One full-leaf exit claim: spends the settled permission-tree leaf, funded
/// by the delegate UTXOs at the covenant deposit address (any depositor's,
/// not the wallet). The payout lands on L1, so the row acks on my L1 UTXO
/// sum rising; there is no on-needs-funding hook because the pool is not my
/// address — a short pool means deposit more via create/join.
export async function submitClaim(opts: {
  identity: Identity;
  client: RpcClient;
  /// Covenant id hex from /api/state.
  covenantId: string;
  /// Deposit P2SH address from /api/state — source of the delegate inputs.
  depositAddress: string;
  root: ExitRoot;
  leaf: ExitLeaf;
  /// My L1 UTXO sum at submit; seeds the payout ack witness.
  balanceBefore: bigint | null;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
}): Promise<string> {
  const { identity, client, covenantId, depositAddress, root, leaf, onActivity } = opts;
  const args = claimArgs(covenantId, root, leaf);

  const { entries } = await client.getUtxosByAddresses({ addresses: [depositAddress] });
  const delegates: WalletUtxo[] = entries.map((e) => ({
    txid_hex: e.outpoint.transactionId,
    index: e.outpoint.index,
    amount: e.amount,
    spk_hex: e.scriptPublicKey.script,
    spk_version: e.scriptPublicKey.version,
  }));
  if (!canAffordClaim(delegates, args.leaf_amount, args.fee)) {
    throw new Error(`delegate pool at ${depositAddress} cannot cover ${kas(args.leaf_amount)} — deposit more via create/join`);
  }
  const utxos = delegates.map((d) => new UtxoCandidate(d.txid_hex, d.index, d.amount, d.spk_hex, d.spk_version));

  const bytes = claim_tx(
    args.covenant_id_hex,
    args.permission_txid_hex,
    args.permission_index,
    args.permission_rent,
    args.old_root_hex,
    args.old_unclaimed,
    args.depth,
    args.leaf_index,
    args.leaf_spk_hex,
    args.leaf_amount,
    args.new_root_hex,
    args.new_unclaimed,
    args.siblings_hex,
    utxos,
    args.fee,
  );
  // The claim burns CLAIM_FEE from the delegate change, so submitTx accepts
  // it into the mempool on any node — the demo L1 and testnet alike.
  const txid = await identity.wallet.submitTx(client, bytes);
  onActivity('claim exits', txid, { kind: 'l1', before: opts.balanceBefore });
  return txid;
}

/// One activity-log row; `status` walks pending → on L2 → settled via the
/// poll-driven chip walker in match.ts.
export interface ActivityRow {
  id: number;
  label: string;
  txid: string;
  status: 'pending' | 'on L2' | 'settled';
  /// What DA change acks this row; drives the chip walk.
  witness?: ActivityWitness;
  /// Settlement txid stamped when the row reached on L2; the settled chip
  /// flips once /api/state later serves a different one.
  settledTxid?: string | null;
}

// ---------------------------------------------------------------------------
// Shared client + balance view.

let clientPromise: Promise<RpcClient> | null = null;

/// The one shared L1 RpcClient for every component outside KeyBar. A failed
/// connect clears the cached promise so the next caller retries instead of
/// every L1 touch staying dead for the session.
export function sharedClient(): Promise<RpcClient> {
  clientPromise ??= connectClient().catch((e) => {
    clientPromise = null;
    throw e;
  });
  return clientPromise;
}

/// My L2 balance and live L1 UTXOs polled every 2 s; `null` while the first
/// poll is in flight, last-good retained on errors. Feeds affordance gates
/// (single-UTXO, via canAfford) and the fund-me hint.
export interface MyBalances {
  utxos: WalletUtxo[] | null;
  l2: bigint | null;
  exists: boolean;
}

export function useMyBalances(identity: Identity | null): MyBalances {
  const [b, setB] = useState<MyBalances>({ utxos: null, l2: null, exists: false });
  useEffect(() => {
    if (!identity) {
      setB({ utxos: null, l2: null, exists: false });
      return;
    }
    let stop = false;
    const tick = async () => {
      try {
        const client = await sharedClient();
        const [account, utxos] = await Promise.all([fetchAccount(identity.userIdHex), identity.wallet.l1Utxos(client)]);
        if (!stop) {
          setB({
            utxos,
            l2: account.exists && account.balance !== undefined ? BigInt(account.balance) : 0n,
            exists: account.exists,
          });
        }
      } catch {
        /* keep last good */
      }
    };
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [identity]);
  return b;
}

// ---------------------------------------------------------------------------
// Formatting.

const SOMPI = 100_000_000n;

/// Sompi → display KAS.
export function kas(amount: number | bigint): string {
  return `${(Number(amount) / Number(SOMPI)).toFixed(2)} KAS`;
}

/// `0x3a…9c` style short form for hashes and ids.
export function shortHex(hex: string): string {
  return `${hex.slice(0, 6)}…${hex.slice(-4)}`;
}
