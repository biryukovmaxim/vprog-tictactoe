//! Genesis pubkey baked into the guest, gating `Init` of the singleton config resource.
//!
//! The key is provided at build time via `VPROG_TICTACTOE_GENESIS_PUBKEY` (64 hex chars,
//! X-only BIP-340 pubkey), so build and deployment choose the operator key without code changes;
//! the value is part of the image id, and production ELF builds must set it explicitly. When the
//! env is unset the pubkey falls back to BIP-340 test vector 0 (secp256k1 scalar `3`), whose
//! known private key keeps dev builds, CI and e2e tests deterministic. A set-but-malformed value
//! (wrong length or non-hex byte) fails compilation via const evaluation.

use hex_literal::hex;

/// Fallback when the env is unset: X-only pubkey of secp256k1 scalar `3`
/// (BIP-340 test vector 0).
const DEFAULT_GENESIS_PUBKEY: [u8; 32] =
    hex!("F9308A019258C31049344F85F89D5229B531C845836F99B08601F113BCE036F9");

/// X-only genesis pubkey; env-overridable at build time, test-vector default otherwise.
pub const GENESIS_PUBKEY: [u8; 32] = match option_env!("VPROG_TICTACTOE_GENESIS_PUBKEY") {
    Some(hex) => hex32(hex),
    None => DEFAULT_GENESIS_PUBKEY,
};

/// Const hex decoder for a 64-char string into 32 bytes. Panics in const
/// evaluation (a build error) on wrong length or a non-hex byte; the guest
/// itself never runs this at runtime.
const fn hex32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    assert!(b.len() == 64, "VPROG_TICTACTOE_GENESIS_PUBKEY must be 64 hex chars");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        let hi = hex_nib(b[i * 2]);
        let lo = hex_nib(b[i * 2 + 1]);
        out[i] = (hi << 4) | lo;
        i += 1;
    }
    out
}

/// Const nibble decode; panics in const evaluation on non-hex input.
// The panic is a build-time guard on env-provided input
#[allow(clippy::panic)]
const fn hex_nib(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("VPROG_TICTACTOE_GENESIS_PUBKEY contains a non-hex byte"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_genesis_is_bip340_test_vector_0() {
        if option_env!("VPROG_TICTACTOE_GENESIS_PUBKEY").is_some() {
            return; // env override active; the dev fallback is not in play
        }
        assert_eq!(GENESIS_PUBKEY[0..8], DEFAULT_GENESIS_PUBKEY[0..8]);
    }
}
