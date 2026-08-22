//! Resolved unlocker buckets: which auth kinds this program admits.
//!
//! The unlocker types are imported from the battery's auth-context module; this file
//! declares the bag itself, so the app decides what it can have as auth. Each
//! bucket is consumed by the matching arm of `LockEnum::unlock`.

use alloc::vec::Vec;

pub use vprogs_zk_backend_risc0_runtime_processor::auth_context::{
    MultisigUnlocker, SchnorrUnlocker,
};

/// Heterogeneous bag of resolved unlockers, one bucket per unlocker type this
/// program's locks consult. Bucket conventions match the battery's: `schnorr`
/// holds one entry per signer sorted by `resource_idx` ascending; `multisig`
/// aggregates contributions to at most one entry per `resource_idx`, in wire
/// order.
#[derive(Default)]
pub struct AuthContext {
    /// Single-key Schnorr-lock authorities.
    pub schnorr: Vec<(u8, SchnorrUnlocker)>,
    /// Aggregated multisig contributions.
    pub multisig: Vec<(u8, MultisigUnlocker)>,
}
