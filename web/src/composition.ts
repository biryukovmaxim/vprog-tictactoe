//! Entry composition for create/join: deposit sizing, affordability, UTXO
//! picking, and the submit flow over the encoder builders. One payload
//! `[Deposit?] + CreateGame|JoinGame` per carrier; the deposit output layout
//! lives entirely inside the encoder — the web only passes `deposit_amount`.

import { useEffect, useState } from 'react';
import { fetchAccount } from './da';
import type { Identity } from './KeyBar';
import { NETWORK, connectClient, type WalletUtxo } from './wallet';
import type { RpcClient } from './kaspa-pkg/kaspa.js';
import { UtxoCandidate, create_game_tx, join_game_tx, network_params } from './wasm/vprog_tictactoe_encoder_wasm.js';

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

/// Whether the wallet can pay `deposit` plus a fee pad out of `l1Available`.
export function canAfford(l1Available: bigint, deposit: bigint, feeEstimate: bigint): boolean {
  return l1Available >= deposit + feeEstimate;
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
  if (!picked) throw new Error(`insufficient L1 funds — fund ${identity.wallet.address}`);
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

/// One activity-log row; `status` walks pending → on L2 → settled as later
/// tasks derive the chips from the DA polls.
export interface ActivityRow {
  id: number;
  label: string;
  txid: string;
  status: 'pending' | 'on L2' | 'settled';
}

// ---------------------------------------------------------------------------
// Shared client + balance view.

let clientPromise: Promise<RpcClient> | null = null;

/// The one shared L1 RpcClient for every component outside KeyBar.
export function sharedClient(): Promise<RpcClient> {
  clientPromise ??= connectClient();
  return clientPromise;
}

/// My L2/L1 balances polled every 2 s; `null` while the first poll is in
/// flight, last-good retained on errors. Feeds affordance gates and banners.
export interface MyBalances {
  l1: bigint | null;
  l2: bigint | null;
  exists: boolean;
}

export function useMyBalances(identity: Identity | null): MyBalances {
  const [b, setB] = useState<MyBalances>({ l1: null, l2: null, exists: false });
  useEffect(() => {
    if (!identity) {
      setB({ l1: null, l2: null, exists: false });
      return;
    }
    let stop = false;
    const tick = async () => {
      try {
        const client = await sharedClient();
        const [account, utxos] = await Promise.all([fetchAccount(identity.userIdHex), identity.wallet.l1Utxos(client)]);
        if (!stop) {
          setB({
            l1: utxos.reduce((sum, u) => sum + u.amount, 0n),
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
