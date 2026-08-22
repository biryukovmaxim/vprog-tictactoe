//! Instruction wire framing: signers, actions, then a free-form tail.
//!
//! ```text
//! ix_data = signers_section || actions_section || tail
//!   signers_section = u32 n_signers || [resource_idx u8 || kind u8 || body]
//!   actions_section = u32 n_actions || [action_tag u8 || body]
//!   tail            = arbitrary bytes (typically schnorr signatures and
//!                     witness preimages referenced by signer pointers)
//! ```
//!
//! Resources (and their `AccessMetadata`) arrive ordered by `resource_id`
//! (asserted at decode in the ABI layer). To let program logic address
//! resources in the order *it* needs (independent of id ordering), every
//! action body carries explicit `u8` index(es) into the resource list; the
//! program's decoder bounds-checks them via [`read_resource_idx`] and rejects
//! malformed actions.
//!
//! The signed-message prefix for a Schnorr signer is
//! `payload.bytes[..end_of_actions]`: i.e. everything up to but not including
//! the tail. Signers commit to access metadata, signer pointers, and actions,
//! but not to the tail bytes (which contain their own signatures).
//!
//! Signers carry the same bounds-checked `resource_idx` as actions, and must appear sorted by it
//! (non-strict; multisig allows multiple signers per resource). Within a resource the wire order
//! must be ascending by pubkey; `decode_ix` enforces the outer ordering and the lock matchers
//! verify the inner one, so no stage re-sorts what the prover supplied.

use alloc::vec::Vec;

use vprogs_core_codec::{Error, Reader, Result as CodecResult};

use crate::runtime::signer::{SignerEnum, decode_signer};

/// Decoded `ix_data`: framing plus the program-decoded actions. `A` carries any
/// borrow lifetimes internally (the action decoder ties them to the input buffer).
pub struct DecodedIx<A> {
    /// Parsed signers paired with their `resource_idx`, each below `n_resources` and sorted
    /// non-strict by it. Within a resource, decode leaves the order as supplied; the lock matchers
    /// require it ascending by pubkey.
    pub signers: Vec<(u8, SignerEnum)>,
    pub actions: Vec<A>,
    /// Byte offset within `ix_data` (NOT `payload.bytes`) where the actions
    /// section ends. The runtime adds the access-metadata-prefix length to
    /// translate this into `payload.bytes` coordinates for the signed prefix.
    pub end_of_actions_in_ix: usize,
}

/// Decodes the instruction stream from `ix_data`. Bytes after the actions
/// section are treated as the tail and remain part of `payload.bytes` for
/// signer offset dereferencing.
pub fn decode_ix<'a, A, F>(
    orig: &'a [u8],
    n_resources: usize,
    decode_action: F,
) -> CodecResult<DecodedIx<A>>
where
    F: FnMut(&mut &'a [u8], usize) -> CodecResult<A>,
{
    let mut bytes: &'a [u8] = orig;
    let mut decode_action = decode_action;

    // Signers: enforce in-range and non-strict ascending by resource_idx during decode.
    let mut prev_resource_idx: Option<u8> = None;
    let signers = bytes.many("ix.signers", |buf: &mut &'a [u8]| {
        let entry = decode_signer(buf)?;
        if entry.0 as usize >= n_resources {
            return Err(Error::Decode("ix.signer: resource_idx out of range"));
        }
        if let Some(p) = prev_resource_idx {
            if entry.0 < p {
                return Err(Error::Decode("ix.signer: resource_idx not ascending"));
            }
        }
        prev_resource_idx = Some(entry.0);
        Ok(entry)
    })?;

    let actions = bytes.many("ix.actions", |buf: &mut &'a [u8]| decode_action(buf, n_resources))?;
    let end_of_actions_in_ix = orig.len() - bytes.len();

    Ok(DecodedIx { signers, actions, end_of_actions_in_ix })
}

/// Reads a `u8` resource index that must reference one of the `n_resources`
/// declared resources. Shared by the signer bounds-check above (via inline
/// check) and exposed for the program's action decoder.
pub fn read_resource_idx(
    buf: &mut &[u8],
    field: &'static str,
    n_resources: usize,
) -> CodecResult<u8> {
    let idx = buf.byte(field)?;
    if (idx as usize) >= n_resources {
        return Err(Error::Decode(field));
    }
    Ok(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An action decoder that never succeeds: framing tests use empty action
    /// sections, so it must never actually run.
    fn reject_action(_buf: &mut &[u8], _n_resources: usize) -> CodecResult<()> {
        Err(Error::Decode("no actions expected in framing tests"))
    }

    fn signer_section(entries: &[(u8, u8, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (resource_idx, kind, body) in entries {
            out.push(*resource_idx);
            out.push(*kind);
            out.extend_from_slice(body);
        }
        out
    }

    /// Single-key Schnorr signer body: 4 bytes (`u32 sig_offset`).
    fn schnorr_signer_body(sig_offset: u32) -> Vec<u8> {
        sig_offset.to_le_bytes().to_vec()
    }

    /// Multisig Schnorr signer body: `u8 pubkey_idx || u32 sig_offset` (5 bytes).
    fn multisig_schnorr_signer_body(pubkey_idx: u8, sig_offset: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity(5);
        v.push(pubkey_idx);
        v.extend_from_slice(&sig_offset.to_le_bytes());
        v
    }

    fn empty_actions_section() -> Vec<u8> {
        0u32.to_le_bytes().to_vec()
    }

    #[test]
    fn decode_signers_happy_path() {
        let mut ix = signer_section(&[(0, 0x01, schnorr_signer_body(100))]);
        ix.extend_from_slice(&empty_actions_section());

        let decoded = decode_ix(&ix, 1, reject_action).unwrap();
        assert_eq!(decoded.signers.len(), 1);
        assert_eq!(decoded.signers[0].0, 0);
        assert_eq!(decoded.actions.len(), 0);
        assert_eq!(decoded.end_of_actions_in_ix, ix.len());
    }

    #[test]
    fn decode_signers_allows_duplicate_resource_idx() {
        // Multisig case: two contributions for the same resource via the
        // dedicated multisig signer kind.
        let mut ix = signer_section(&[
            (0, 0x03, multisig_schnorr_signer_body(0, 100)),
            (0, 0x03, multisig_schnorr_signer_body(1, 200)),
        ]);
        ix.extend_from_slice(&empty_actions_section());

        let decoded = decode_ix(&ix, 1, reject_action).unwrap();
        assert_eq!(decoded.signers.len(), 2);
    }

    #[test]
    fn decode_signers_rejects_out_of_order_resource_idx() {
        let mut ix = signer_section(&[
            (1, 0x01, schnorr_signer_body(100)),
            (0, 0x01, schnorr_signer_body(200)),
        ]);
        ix.extend_from_slice(&empty_actions_section());

        // Both indices are in range, so this can only fail on the ordering rule.
        assert!(matches!(
            decode_ix(&ix, 2, reject_action),
            Err(Error::Decode(m)) if m.contains("not ascending")
        ));
    }

    #[test]
    fn decode_signers_rejects_out_of_range_resource_idx() {
        let mut ix = signer_section(&[(1, 0x01, schnorr_signer_body(100))]);
        ix.extend_from_slice(&empty_actions_section());

        assert!(matches!(
            decode_ix(&ix, 1, reject_action),
            Err(Error::Decode(m)) if m.contains("out of range")
        ));
    }

    #[test]
    fn decode_signers_rejects_unknown_kind() {
        let mut ix = signer_section(&[(0, 0xEE, vec![0u8; 5])]);
        ix.extend_from_slice(&empty_actions_section());
        assert!(decode_ix(&ix, 1, reject_action).is_err());
    }

    #[test]
    fn decode_allows_tail_bytes_after_actions() {
        let mut ix = 0u32.to_le_bytes().to_vec(); // signers
        ix.extend_from_slice(&0u32.to_le_bytes()); // actions
        let end = ix.len();
        ix.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // tail blob

        let decoded = decode_ix(&ix, 0, reject_action).unwrap();
        assert_eq!(decoded.end_of_actions_in_ix, end);
    }

    /// The program's action decoder runs inside the framing loop: a decode
    /// error in any action rejects the whole ix.
    #[test]
    fn action_decoder_error_rejects_ix() {
        let mut ix = 0u32.to_le_bytes().to_vec(); // signers
        ix.extend_from_slice(&1u32.to_le_bytes()); // one action...
        ix.extend_from_slice(&[0x01, 0x00]); // ...whose bytes the stub rejects

        assert!(decode_ix(&ix, 1, reject_action).is_err());
    }
}
