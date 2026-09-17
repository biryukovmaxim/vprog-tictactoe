//! Pure claim-exit helpers: the my-key spk filter over served exit leaves,
//! the full-deduct `claim_tx` argument assembly from the served shapes, and
//! the delegate-pool affordance. Framework-free by design (no React imports)
//! — components live in the .tsx files; the wasm call lives in composition.

import type { RpcClient } from 'kaspa-wasm';
import type { ExitLeaf, ExitRoot } from './da';
import type { WalletUtxo } from './wallet';

/// Schnorr P2PK script bytes hex (`OpData32 | pubkey | OpCheckSig`, 34
/// bytes) — the `StandardSpk::PubKey` layout exit leaves carry in `spk_hex`.
export function mySpkHex(pubkeyHex: string): string {
  return `20${pubkeyHex}ac`;
}

/// One claimable exit: a settled root plus the leaf of it paying my key.
export interface ClaimTarget {
  /// `root:index` — identifies the leaf across polls.
  key: string;
  root: ExitRoot;
  leaf: ExitLeaf;
}

/// Leaves worth showing a Claim button for: unspent and paying my key. Every
/// root /api/exits serves is a materialized settled root (the settled gate is
/// the endpoint itself), and the served record advances to the claim's
/// continuation after each spend, so remaining leaves keep fresh args
/// (old_root, permission outpoint, full_claim) until their own `spent` entry
/// lands: claims chain sequentially within a root.
export function claimableExits(roots: ExitRoot[], pubkeyHex: string): ClaimTarget[] {
  const spk = mySpkHex(pubkeyHex);
  return roots.flatMap((root) =>
    root.leaves
      .filter((leaf) => leaf.spent === null && leaf.spk_hex === spk)
      .map((leaf) => ({ key: `${root.root}:${leaf.index}`, root, leaf })),
  );
}

/// `claim_tx` arguments assembled from the served shapes; field names mirror
/// the encoder-wasm parameter names. The DA precomputes the post-claim root
/// per leaf, so no merkle math happens here. The deduct is always the full
/// leaf amount (demo lock: full claims only; the encoder pins it). The fee
/// is absent: it prices from the node's feerate estimation at submit time
/// (see `claimFee`) and burns from the claimer's own collateral input.
export interface ClaimArgs {
  covenant_id_hex: string;
  permission_txid_hex: string;
  permission_index: number;
  permission_rent: bigint;
  old_root_hex: string;
  old_unclaimed: bigint;
  depth: number;
  leaf_index: number;
  leaf_spk_hex: string;
  leaf_amount: bigint;
  new_root_hex: string;
  new_unclaimed: bigint;
  siblings_hex: string[];
}

/// Maps one settled leaf plus its root into the encoder `claim_tx` arguments.
export function claimArgs(covenantIdHex: string, root: ExitRoot, leaf: ExitLeaf): ClaimArgs {
  return {
    covenant_id_hex: covenantIdHex,
    permission_txid_hex: root.settlement_txid,
    permission_index: root.outpoint_index,
    permission_rent: BigInt(root.rent),
    old_root_hex: root.root,
    old_unclaimed: BigInt(root.unclaimed),
    depth: leaf.siblings.length,
    leaf_index: leaf.index,
    leaf_spk_hex: leaf.spk_hex,
    leaf_amount: BigInt(leaf.amount),
    new_root_hex: leaf.full_claim.new_root,
    new_unclaimed: BigInt(leaf.full_claim.new_unclaimed),
    siblings_hex: leaf.siblings,
  };
}

/// The claim fee from the node's feerate estimation: the priority bucket's
/// feerate (sompi/gram) times the tx byte length plus one sigop compute mass
/// (~10k grams; byte length alone under-prices once the collateral P2PK
/// signature is added). Over-estimating slightly is fine: the unburned
/// remainder returns as the collateral change.
export async function claimFee(client: RpcClient, txBytes: Uint8Array): Promise<bigint> {
  const { estimate } = await client.getFeeEstimate();
  const feerate = estimate.priorityBucket.feerate;
  return BigInt(Math.ceil(feerate * (txBytes.length + 10_000)));
}

/// Whether the delegate pool can fund the payout: claims aggregate inputs,
/// so the SUM covering `amount` affords, unlike carriers, whose single-UTXO
/// spend requires `pickUtxo`'s largest-strictly-covering rule. The fee never
/// comes from the pool: it burns from the claimer's own collateral input.
export function canAffordClaim(utxos: WalletUtxo[], leafAmount: bigint): boolean {
  let total = 0n;
  for (const u of utxos) total += u.amount;
  return total >= leafAmount;
}
