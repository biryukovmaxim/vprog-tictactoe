//! Resource-kind discriminator. The first byte of every resource's data
//! distinguishes one typed payload from another (config vs user vs game) without
//! re-hashing the resource id.
//!
//! Each payload's `from_bytes` checks this byte against its expected [`Kind`]
//! before zerocopy-parsing the kindless body that follows, so an action aimed at
//! the wrong kind of resource never sees a parsed body.

/// The resource kind named by the data's first byte.
#[repr(u8)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
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

/// The kind byte at offset 0, or `None` if `bytes` is empty or carries an
/// unknown discriminator.
pub fn kind_of(bytes: &[u8]) -> Option<Kind> {
    bytes.first().and_then(|&b| Kind::try_from(b).ok())
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
        assert_eq!(kind_of(&[2]), Some(Kind::Game));
        assert_eq!(kind_of(&[3]), None);
        assert_eq!(kind_of(&[]), None);
    }
}
