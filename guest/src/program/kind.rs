//! Resource-kind discriminator. The first byte of every resource's data
//! distinguishes one typed payload from another (config vs user vs game) without
//! re-hashing the resource id.
//!
//! Views in `crate::program::resource_ext` read this byte to dispatch into the
//! typed view; apply functions in `crate::program::action` use it to reject ix that
//! aimed an action at the wrong kind of resource.
//!
//! The [`KIND_*`] consts are the wire values used by that byte-level dispatch; the
//! typed enums below carry the same values into the resource layouts, where zerocopy
//! checks the discriminant at parse time. `Unset = 0` exists only in the all-zero
//! construction state (a `write_*` starting from zeroed bytes); the views reject it,
//! so it never persists.

use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout, Unaligned};

pub const KIND_CONFIG: u8 = 0;
pub const KIND_USER: u8 = 1;
pub const KIND_GAME: u8 = 2;

/// Discriminator of the config payload. Its single variant is the all-zero value,
/// so a zeroed construction buffer already carries it.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    FromZeros,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum ConfigKind {
    Config = 0,
}

/// Discriminator of the user payload.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    FromZeros,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum UserKind {
    /// All-zero construction state; never persists.
    Unset = 0,
    User = 1,
}

/// Discriminator of the game payload.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    FromZeros,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum GameKind {
    /// All-zero construction state; never persists.
    Unset = 0,
    Game = 2,
}

/// Returns the kind byte at offset 0, or `None` if `bytes` is empty.
pub fn kind_of(bytes: &[u8]) -> Option<u8> {
    bytes.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dispatch consts and the enum discriminants are two spellings of the same wire
    /// values; this pins them together.
    #[test]
    fn consts_match_enum_discriminants() {
        assert_eq!(ConfigKind::Config as u8, KIND_CONFIG);
        assert_eq!(UserKind::User as u8, KIND_USER);
        assert_eq!(GameKind::Game as u8, KIND_GAME);
        assert_eq!(UserKind::Unset as u8, 0);
        assert_eq!(GameKind::Unset as u8, 0);
    }
}
