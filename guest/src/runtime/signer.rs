//! Signer enum + dispatch: which signer kinds can satisfy this program's locks.
//!
//! The standard mechanisms' structs and `Signer` impls are imported from the battery's
//! `signer_variants`; the env-genesis variant is app-defined in
//! [`crate::runtime::signer_variants`]. This file defines the app's `SignerEnum` over them and the
//! kind-byte demux. Adding a signer kind means adding a variant here, never editing vprogs.

use vprogs_core_codec::{Error, Reader, Result as CodecResult};
use vprogs_zk_backend_risc0_runtime_processor::signer_trait::Signer;
// The app's supported signer set: battery impls for witness kinds.
pub use vprogs_zk_backend_risc0_runtime_processor::signer_variants::{
    MultisigPrevTxV1WitnessSigner, PrevTxV1WitnessSigner,
};

use crate::runtime::ix::read_resource_idx;
// Lock-reading and env-genesis variants are app-defined to match this app's resource wire layouts.
pub use crate::runtime::signer_variants::{
    GenesisSchnorrSigPtrSigner, MultisigSchnorrSigPtrSigner, SchnorrSigPtrSigner,
};

/// All known signer kinds. Each variant's `resolve` produces an `Unlocker` of
/// some concrete type; `program::run::resolve_signers` routes the result into the
/// matching `AuthContext` bucket.
pub enum SignerEnum {
    SchnorrSigPtr(SchnorrSigPtrSigner),
    PrevTxV1Witness(PrevTxV1WitnessSigner),
    MultisigSchnorrSigPtr(MultisigSchnorrSigPtrSigner),
    MultisigPrevTxV1Witness(MultisigPrevTxV1WitnessSigner),
    GenesisSchnorrSigPtr(GenesisSchnorrSigPtrSigner),
}

/// Decodes a single signer entry: `(resource_idx u8 || kind u8 || body)`.
/// Returns `(resource_idx, signer)`; `resource_idx` lives outside the body
/// because it's a shared field every signer carries, and is bounds-checked
/// against `n_resources` via [`read_resource_idx`].
pub fn decode_signer(buf: &mut &[u8], n_resources: usize) -> CodecResult<(u8, SignerEnum)> {
    let resource_idx = read_resource_idx(buf, "signer.resource_idx", n_resources)?;
    let kind = buf.byte("signer.kind")?;
    let body = match kind {
        SchnorrSigPtrSigner::TAG => SignerEnum::SchnorrSigPtr(SchnorrSigPtrSigner::decode(buf)?),
        PrevTxV1WitnessSigner::TAG => {
            SignerEnum::PrevTxV1Witness(PrevTxV1WitnessSigner::decode(buf)?)
        }
        MultisigSchnorrSigPtrSigner::TAG => {
            SignerEnum::MultisigSchnorrSigPtr(MultisigSchnorrSigPtrSigner::decode(buf)?)
        }
        MultisigPrevTxV1WitnessSigner::TAG => {
            SignerEnum::MultisigPrevTxV1Witness(MultisigPrevTxV1WitnessSigner::decode(buf)?)
        }
        GenesisSchnorrSigPtrSigner::TAG => {
            SignerEnum::GenesisSchnorrSigPtr(GenesisSchnorrSigPtrSigner::decode(buf)?)
        }
        _ => return Err(Error::Decode("signer: unknown kind")),
    };
    Ok((resource_idx, body))
}
