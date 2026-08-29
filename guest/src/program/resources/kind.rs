//! Resource-kind discriminator. The first byte of every resource's data
//! distinguishes one typed payload from another (config vs user vs game) without
//! re-hashing the resource id.
//!
//! Each payload's `from_bytes` splits this byte off with zerocopy's
//! `try_from_prefix` (parse plus split in one step) and rejects it unless it
//! names the expected [`Kind`], before zerocopy-parsing the kindless body that
//! follows; an action aimed at the wrong kind of resource never sees a parsed
//! body.

use zerocopy::{Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned};

/// The resource kind named by the data's first byte.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    TryFromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    Unaligned
)]
pub enum Kind {
    Config = 0,
    User = 1,
    Game = 2,
}

impl TryFrom<u8> for Kind {
    type Error = ();

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::Config),
            1 => Ok(Self::User),
            2 => Ok(Self::Game),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant maps to exactly one wire byte and back; the gaps around
    /// the defined values reject. A new variant added without a `TryFrom` arm
    /// fails here.
    #[test]
    fn kind_byte_round_trips_every_variant() {
        for kind in [Kind::Config, Kind::User, Kind::Game] {
            assert_eq!(Kind::try_from(kind as u8), Ok(kind));
        }
        assert!(Kind::try_from(3).is_err());
        assert!(Kind::try_from(0xFF).is_err());
    }

    /// The zerocopy parse accepts exactly the same byte set as `TryFrom<u8>`,
    /// so prefix splitting and byte conversion cannot disagree.
    #[test]
    fn zerocopy_parse_matches_try_from() {
        for b in 0..=3u8 {
            let parsed = Kind::try_ref_from_bytes(&[b]).is_ok();
            assert_eq!(parsed, Kind::try_from(b).is_ok(), "byte {b}");
        }
    }
}
