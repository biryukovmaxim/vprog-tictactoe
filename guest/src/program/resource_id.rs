//! Domain-separated resource id derivations: one per resource kind, each under its own
//! [`Domain`] tag (see `crate::program::domain`), so the three keyspaces are uncollidable
//! regardless of seed content.
//!
//! - **Config** (singleton): seeded by a fixed label.
//! - **User**: seeded by the 32-byte identity hash of the initial lock controlling the resource.
//! - **Game**: seeded by the creator's `initial_lock_hash` plus their `games_started` counter
//!   (carried by the user resource): sequential, deterministic ids: the Nth game someone starts
//!   always derives the same id, and two creators' games never collide.

use vprogs_core_types::ResourceId;
use vprogs_zk_backend_risc0_api::{Hasher, Sha256};

use crate::program::domain::Domain;

/// The singleton config resource id (`label = "config"`).
pub fn config_resource_id() -> ResourceId {
    ResourceId::from(Sha256::hash_with_domain(&[Domain::Config as u8], b"config"))
}

/// Derives a user-resource id from the 32-byte identity hash of its initial
/// lock. The lock identity is `LockEnum::id_hash()`; see the battery's `lock_trait`.
pub fn derive_user_resource(initial_lock_hash: &[u8; 32]) -> ResourceId {
    ResourceId::from(Sha256::hash_with_domain(&[Domain::User as u8], initial_lock_hash))
}

/// Derives the id of the creator's `game_number`-th game (0-based): the counter is the
/// `games_started` value read from the creator's user resource at `CreateGame` time, so the id
/// is deterministic across replays and unique per creator.
pub fn derive_game_resource(creator_initial_lock_hash: &[u8; 32], game_number: u64) -> ResourceId {
    ResourceId::from(Sha256::hash_with_domain(
        &[Domain::Game as u8],
        [creator_initial_lock_hash.as_slice(), &game_number.to_le_bytes()].concat(),
    ))
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    #[test]
    fn config_id_matches_sha256_with_domain() {
        let want = Sha256::hash_with_domain(&[Domain::Config as u8], b"config");
        assert_eq!(*config_resource_id(), want);
    }

    #[test]
    fn user_id_changes_with_seed() {
        let a = derive_user_resource(&[0x11u8; 32]);
        let b = derive_user_resource(&[0x12u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn game_id_changes_with_creator_and_number() {
        let ilh = [0x21u8; 32];
        assert_ne!(derive_game_resource(&ilh, 0), derive_game_resource(&ilh, 1));
        assert_ne!(derive_game_resource(&ilh, 0), derive_game_resource(&[0x22u8; 32], 0));
    }

    /// The game counter joins the seed length-prefixed-free but fixed-width (u64 LE), so
    /// `(ilh, n)` pairs are injective: no shifted-split aliasing between the two fields.
    #[test]
    fn game_seed_split_is_unambiguous() {
        let mut seed = Vec::new();
        seed.extend_from_slice(&[0xABu8; 32]);
        seed.extend_from_slice(&7u64.to_le_bytes());
        let want = Sha256::hash_with_domain(&[Domain::Game as u8], seed);
        assert_eq!(*derive_game_resource(&[0xABu8; 32], 7), want);
    }

    #[test]
    fn kind_keyspaces_are_disjoint_for_same_seed() {
        // Distinct domain tags must produce different ids for identical seed bytes.
        let seed = [0xCDu8; 32];
        let config = Sha256::hash_with_domain(&[Domain::Config as u8], seed);
        let user = Sha256::hash_with_domain(&[Domain::User as u8], seed);
        let game = Sha256::hash_with_domain(&[Domain::Game as u8], seed);
        assert_ne!(config, user);
        assert_ne!(config, game);
        assert_ne!(user, game);
    }
}
