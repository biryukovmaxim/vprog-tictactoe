//! One-byte SHA-256 domain tags for this app's hash derivations: one domain per resource kind,
//! so the derivation and the kind are the same partition.
//!
//! Each tag is the leading byte of a domain-separated SHA-256 input, so two derivations with the
//! same payload but different domains can never collide.
#[repr(u8)]
pub enum Domain {
    /// Reserved: the battery's signer-message digest prefix (`compute_sig_message`).
    /// Not used by app derivations.
    SigMessage = 2,
    /// Reserved: the battery's lock-identity hash prefix (`Lock::id_hash`).
    /// Not used by app derivations.
    LockId = 3,

    /// Config-resource id derivation (`config_resource_id`).
    Config = 4,
    /// Game-resource id derivation (`derive_game_resource`).
    Game = 5,
    /// User-resource id derivation (`derive_user_resource`).
    User = 6,
}
