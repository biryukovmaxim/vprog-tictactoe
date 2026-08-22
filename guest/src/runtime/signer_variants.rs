//! App-defined signer variants.
//!
//! The standard signer mechanisms (schnorr-sig-pointer, prev-tx witness, and their multisig
//! flavours) are imported from the battery's `signer_variants` and used through the battery's
//! `Signer` impls.

use vprogs_core_codec::{Error, Reader, Result as CodecResult};
use vprogs_zk_backend_risc0_runtime_processor::{
    auth::verify_k256_schnorr_sig,
    auth_context::SchnorrUnlocker,
    signer_trait::{Signer, SignerResolveContext},
};

use crate::runtime::genesis::GENESIS_PUBKEY;

/// Genesis-authority Schnorr signer: resolves to a `SchnorrUnlocker` carrying this guest's
/// env-provided [`GENESIS_PUBKEY`] rather than a pubkey read from the target resource's
/// lock. Body: `u32 sig_offset`, offset into `payload_bytes` of the 64-byte BIP-340 signature.
///
/// Reading no lock lets it authorize a resource whose slot is still empty. Nothing in `resolve`
/// ties it to a specific action or resource; the effective restriction to `Init` is emergent, since
/// `apply_init` is the only action that authorizes against a resource with no readable lock. The
/// genesis key signs the runtime sig-message directly, with no on-chain UTXO / witness coupling.
pub struct GenesisSchnorrSigPtrSigner {
    pub sig_offset: u32,
}

impl<'a> Signer<'a> for GenesisSchnorrSigPtrSigner {
    const TAG: u8 = 0x05;
    type Unlocker = SchnorrUnlocker;

    fn decode(buf: &mut &'a [u8]) -> CodecResult<Self> {
        let sig_offset = buf.le_u32("signer.genesis_schnorr.sig_offset")?;
        Ok(Self { sig_offset })
    }

    fn resolve(
        &self,
        resource_idx: u8,
        ctx: &SignerResolveContext<'a>,
    ) -> CodecResult<SchnorrUnlocker> {
        ctx.resources
            .get(resource_idx as usize)
            .ok_or(Error::Decode("signer.genesis_schnorr: resource_idx out of range"))?;
        let sig = read_sig_at_offset(self.sig_offset, ctx)?;
        if !verify_k256_schnorr_sig(&GENESIS_PUBKEY, sig, ctx.sig_msg()) {
            return Err(Error::Decode("signer.genesis_schnorr: invalid signature"));
        }
        Ok(SchnorrUnlocker { pubkey: GENESIS_PUBKEY })
    }
}

/// Reads `payload_bytes[sig_offset..sig_offset+64]` as a 64-byte signature. Local twin of the
/// battery's private helper (its signer variants carry their own).
fn read_sig_at_offset<'a, 'ctx>(
    sig_offset: u32,
    ctx: &'ctx SignerResolveContext<'a>,
) -> CodecResult<&'ctx [u8; 64]> {
    let off = sig_offset as usize;
    ctx.payload_bytes
        .get(off..)
        .and_then(|b| b.first_chunk::<64>())
        .ok_or(Error::Decode("signer: sig_offset out of range"))
}
