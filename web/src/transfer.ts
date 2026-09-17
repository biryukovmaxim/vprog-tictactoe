//! Pure transfer/withdraw helpers: dest-lock derivation (the schnorr lock the
//! guest binds a newborn account to), the create-vs-plain encoder branch,
//! KAS amount parsing, and the withdrawal floor. Framework-free by design
//! (no React imports) — components live in the .tsx files.

/// Battery `Domain::LockId` — the leading byte of every `Lock::id_hash`
/// (vprogs runtime-processor lock_trait.rs).
const LOCK_ID_DOMAIN = 3;
/// Battery `SchnorrLockView::TAG` — the lock-kind byte after the domain.
const SCHNORR_TAG = 1;
/// Guest `Domain::User` — user-resource id derivation over a lock hash
/// (guest resources/domain.rs; pinned in composition.test.ts).
const USER_DOMAIN = 6;

async function sha256(input: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', input));
}

function hexBytes(hex: string): Uint8Array {
  if (!/^[0-9a-fA-F]+$/.test(hex) || hex.length % 2 !== 0) throw new Error(`invalid hex: ${hex}`);
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return out;
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/// Lock identity hash of a dest x-only pubkey: `sha256(0x03 || 0x01 || pubkey)`,
/// the battery `Lock::id_hash` a transfer-create binds the new account to.
/// Pinned against `my_ids().lock_hash_hex` in transfer.test.ts.
export async function destLockHash(pubkeyHex: string): Promise<string> {
  const pubkey = hexBytes(pubkeyHex);
  return toHex(await sha256(Uint8Array.of(LOCK_ID_DOMAIN, SCHNORR_TAG, ...pubkey)));
}

/// User-resource id a dest pubkey derives: `sha256(0x06 || destLockHash)` —
/// lets the form check that the typed dest id and pubkey name the same
/// account before funding a create transfer.
export async function destUserResource(pubkeyHex: string): Promise<string> {
  const lock = await destLockHash(pubkeyHex);
  return toHex(await sha256(Uint8Array.of(USER_DOMAIN, ...hexBytes(lock))));
}

/// Encoder-branch args for one transfer: plain when the dest account exists
/// (any supplied pubkey is dropped), create otherwise — which requires the
/// dest pubkey to bind the newborn lock.
export function transferArgs(dest: { exists: boolean; pubkeyHex?: string }): {
  dest_exists: boolean;
  dest_pubkey_hex?: string;
} {
  if (!dest.exists && !dest.pubkeyHex) throw new Error('dest pubkey required to create a missing account');
  return dest.exists ? { dest_exists: true } : { dest_exists: false, dest_pubkey_hex: dest.pubkeyHex };
}

/// Display KAS → sompi; null when not a positive amount.
export function parseSompi(kas: string): bigint | null {
  const n = Number(kas);
  if (!Number.isFinite(n) || n <= 0) return null;
  return BigInt(Math.round(n * 100_000_000));
}

/// Whether `amount` clears the config `min_withdrawal_amount` floor
/// (sompi; an absent floor constrains nothing).
export function meetsWithdrawFloor(amount: bigint, min: number | undefined): boolean {
  return min === undefined ? true : amount >= BigInt(min);
}
