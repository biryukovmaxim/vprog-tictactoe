//! App-defined signer variants.
//!
//! The witness mechanisms are imported from the battery's `signer_variants`;
//! the lock-reading mechanisms and env-genesis variant are app-defined to decode against
//! this app's resource layouts ([`ConfigBody`], [`UserBody`]).

use vprogs_core_codec::{Error, Reader, Result as CodecResult};
use vprogs_zk_backend_risc0_runtime_processor::{
    auth::verify_k256_schnorr_sig,
    auth_context::{MultisigUnlocker, SchnorrUnlocker},
    signer_trait::{Signer, SignerResolveContext},
};

use crate::{
    program::resources::{config::ConfigBody, kind::Kind, user::UserBody},
    runtime::{
        genesis::GENESIS_PUBKEY,
        lock::{LockEnum, SchnorrLockView},
    },
};

/// Genesis-authority Schnorr signer: resolves to a `SchnorrUnlocker` carrying this guest's
/// env-provided [`GENESIS_PUBKEY`] rather than a pubkey read from the target resource's
/// lock. Body: `u32 sig_offset`, offset into `payload_bytes` of the 64-byte BIP-340 signature.
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

/// Schnorr signature pointer signer decoding against this app's resource wire layouts.
pub struct SchnorrSigPtrSigner {
    pub sig_offset: u32,
}

impl<'a> Signer<'a> for SchnorrSigPtrSigner {
    const TAG: u8 = 0x01;
    type Unlocker = SchnorrUnlocker;

    fn decode(buf: &mut &'a [u8]) -> CodecResult<Self> {
        let sig_offset = buf.le_u32("signer.schnorr.sig_offset")?;
        Ok(Self { sig_offset })
    }

    fn resolve(
        &self,
        resource_idx: u8,
        ctx: &SignerResolveContext<'a>,
    ) -> CodecResult<SchnorrUnlocker> {
        let pubkey = read_schnorr_lock_pubkey(resource_idx, ctx)?;
        let sig = read_sig_at_offset(self.sig_offset, ctx)?;
        if !verify_k256_schnorr_sig(&pubkey, sig, ctx.sig_msg()) {
            return Err(Error::Decode("signer.schnorr: invalid signature"));
        }
        Ok(SchnorrUnlocker { pubkey })
    }
}

/// Multisig Schnorr signature pointer signer decoding against this app's resource wire layouts.
pub struct MultisigSchnorrSigPtrSigner {
    pub pubkey_idx: u8,
    pub sig_offset: u32,
}

impl<'a> Signer<'a> for MultisigSchnorrSigPtrSigner {
    const TAG: u8 = 0x03;
    type Unlocker = MultisigUnlocker;

    fn decode(buf: &mut &'a [u8]) -> CodecResult<Self> {
        Ok(Self {
            pubkey_idx: buf.byte("signer.multisig_schnorr.pubkey_idx")?,
            sig_offset: buf.le_u32("signer.multisig_schnorr.sig_offset")?,
        })
    }

    fn resolve(
        &self,
        resource_idx: u8,
        ctx: &SignerResolveContext<'a>,
    ) -> CodecResult<MultisigUnlocker> {
        let pubkey = read_multisig_lock_pubkey_at(resource_idx, self.pubkey_idx, ctx)?;
        let sig = read_sig_at_offset(self.sig_offset, ctx)?;
        if !verify_k256_schnorr_sig(&pubkey, sig, ctx.sig_msg()) {
            return Err(Error::Decode("signer.multisig_schnorr: invalid signature"));
        }
        Ok(MultisigUnlocker { pubkeys: alloc::vec![pubkey] })
    }
}

fn read_schnorr_lock_pubkey(
    resource_idx: u8,
    ctx: &SignerResolveContext<'_>,
) -> CodecResult<[u8; 32]> {
    let resource = ctx
        .resources
        .get(resource_idx as usize)
        .ok_or(Error::Decode("signer: resource_idx out of range"))?;
    let data = resource.data();
    let kind = data.first().copied().and_then(|b| Kind::try_from(b).ok());
    let lock = match kind {
        Some(Kind::Config) => ConfigBody::from_bytes(data).map_err(Error::Decode)?.lock(),
        Some(Kind::User) => UserBody::from_bytes(data).map_err(Error::Decode)?.lock(),
        _ => return Err(Error::Decode("signer: unknown or absent resource kind")),
    };
    match lock {
        LockEnum::Schnorr(SchnorrLockView { pubkey }) => Ok(*pubkey),
        _ => Err(Error::Decode("signer.schnorr: target resource is not a Schnorr lock")),
    }
}

fn read_multisig_lock_pubkey_at(
    resource_idx: u8,
    pubkey_idx: u8,
    ctx: &SignerResolveContext<'_>,
) -> CodecResult<[u8; 32]> {
    let resource = ctx
        .resources
        .get(resource_idx as usize)
        .ok_or(Error::Decode("signer: resource_idx out of range"))?;
    let data = resource.data();
    let kind = data.first().copied().and_then(|b| Kind::try_from(b).ok());
    let lock = match kind {
        Some(Kind::Config) => ConfigBody::from_bytes(data).map_err(Error::Decode)?.lock(),
        Some(Kind::User) => UserBody::from_bytes(data).map_err(Error::Decode)?.lock(),
        _ => return Err(Error::Decode("signer: unknown or absent resource kind")),
    };
    match lock {
        LockEnum::Multisig(m) => {
            let pk = m
                .iter_pubkeys()
                .nth(pubkey_idx as usize)
                .ok_or(Error::Decode("signer.multisig: pubkey_idx out of range"))?;
            Ok(*pk)
        }
        _ => Err(Error::Decode("signer.multisig: target resource is not a Multisig lock")),
    }
}

/// Reads `payload_bytes[sig_offset..sig_offset+64]` as a 64-byte signature.
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
