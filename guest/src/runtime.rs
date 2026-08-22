pub mod auth_context;
pub mod genesis;
pub mod ix;
pub mod lock;
pub mod lock_codec;
pub mod signer;
pub mod signer_variants;

/// Re-exported so app code has one obvious battery import site and call sites
/// stay short.
pub use vprogs_zk_backend_risc0_runtime_processor::{
    auth, lifecycle, lock_trait, signer_trait, tx_inputs,
};

/// The battery's generic apply context with this program's auth context as the
/// type parameter.
pub type ApplyContext<'a, 'cx> = vprogs_zk_backend_risc0_runtime_processor::action::ApplyContext<
    'a,
    'cx,
    auth_context::AuthContext,
>;
