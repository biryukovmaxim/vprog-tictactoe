//! User resource (`derive_user_resource(initial_lock_hash)`).
//!
//! Wire layout: kind byte + fixed header + tag-driven variable body:
//! ```text
//! [0]       kind                (KIND_USER = 1; see `crate::program::kind`)
//! [1..9]    balance             (u64 LE)
//! [9..17]   games_started       (u64 LE; game-id derivation seed)
//! [17..25]  games_won           (u64 LE)
//! [25..33]  games_finished      (u64 LE)
//! [33..65]  initial_lock_hash   ([u8; 32])
//! [65]      lock_tag            (current lock, may differ from initial)
//! [66..]    lock_body           (length and shape implied by tag)
//! ```
//!

use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned, little_endian::U64 as Le64,
};

use crate::{
    program::kind::KIND_USER,
    runtime::{
        lock::LockEnum,
        lock_codec::{decode_lock_body_unchecked, validate_lock_body},
    },
};

/// Fixed-header byte length: `kind (u8) || balance (u64 LE) || games_started (u64 LE) ||
/// games_won (u64 LE) || games_finished (u64 LE) || initial_lock_hash ([u8; 32]) || lock_tag (u8)`.
/// Derived from `UserRaw` so the sum can never drift from the struct.
pub const USER_HEADER_LEN: usize = core::mem::offset_of!(UserRaw, lock_tag) + 1;

/// Per-player game counters carried by the user resource.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GameStats {
    /// Games ever started; seeds the game-resource id derivation.
    pub started: u64,
    /// Games won (settled in the player's favor).
    pub won: u64,
    /// Games finished (any outcome).
    pub finished: u64,
}

/// Zerocopy DST: kind discriminator + fixed header + tag-driven variable body.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct UserRaw {
    pub kind: u8,
    pub balance: Le64,
    pub games_started: Le64,
    pub games_won: Le64,
    pub games_finished: Le64,
    pub initial_lock_hash: [u8; 32],
    pub lock_tag: u8,
    pub lock_body: [u8],
}

/// Read-only view over a user resource. Body shape and kind validated at
/// `from_bytes` time, so accessors are infallible.
pub struct UserView<'a>(&'a UserRaw);

impl<'a> UserView<'a> {
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.len() < USER_HEADER_LEN {
            return Err("user: too short for header");
        }
        let raw = UserRaw::ref_from_bytes(bytes).map_err(|_| "user: invalid layout")?;
        if raw.kind != KIND_USER {
            return Err("user: wrong kind byte");
        }
        validate_lock_body(raw.lock_tag, &raw.lock_body)?;
        Ok(Self(raw))
    }

    pub fn balance(&self) -> u64 {
        self.0.balance.get()
    }

    /// Games this player ever started; seeds the game-resource id derivation.
    pub fn games_started(&self) -> u64 {
        self.0.games_started.get()
    }

    /// Games this player has won (settled in their favor).
    pub fn games_won(&self) -> u64 {
        self.0.games_won.get()
    }

    /// Games this player has finished (any outcome).
    pub fn games_finished(&self) -> u64 {
        self.0.games_finished.get()
    }

    /// The game counters as one value.
    pub fn stats(&self) -> GameStats {
        GameStats {
            started: self.0.games_started.get(),
            won: self.0.games_won.get(),
            finished: self.0.games_finished.get(),
        }
    }

    pub fn initial_lock_hash(&self) -> &'a [u8; 32] {
        &self.0.initial_lock_hash
    }

    pub fn lock_tag(&self) -> u8 {
        self.0.lock_tag
    }

    /// Returns the typed lock view over the (current) body bytes.
    ///
    /// Infallible by construction: `from_bytes` validated this (tag, body) pair.
    pub fn lock(&self) -> LockEnum<'a> {
        decode_lock_body_unchecked(self.0.lock_tag, &self.0.lock_body)
    }
}

/// Mutable view for fixed-field updates. The lock body can be rewritten via
/// `lock_body_mut`; caller is responsible for keeping tag-implied invariants
/// intact. `initial_lock_hash` is *not* exposed mutably; it's permanent.
pub struct UserViewMut<'a>(&'a mut UserRaw);

impl<'a> UserViewMut<'a> {
    pub fn from_bytes_mut(bytes: &'a mut [u8]) -> Result<Self, &'static str> {
        if bytes.len() < USER_HEADER_LEN {
            return Err("user: too short for header");
        }
        let raw = UserRaw::mut_from_bytes(bytes).map_err(|_| "user: invalid layout")?;
        if raw.kind != KIND_USER {
            return Err("user: wrong kind byte");
        }
        validate_lock_body(raw.lock_tag, &raw.lock_body)?;
        Ok(Self(raw))
    }

    /// Mutable handle to the on-disk balance. The returned `Le64` already
    /// owns the get/set API (`.get() -> u64`, `.set(u64)`); combining read
    /// and write through one borrow avoids the get-then-set ceremony.
    pub fn balance_mut(&mut self) -> &mut Le64 {
        &mut self.0.balance
    }

    /// Mutable handle to the games-started counter (game-id derivation seed).
    pub fn games_started_mut(&mut self) -> &mut Le64 {
        &mut self.0.games_started
    }

    /// Mutable handle to the games-won stat.
    pub fn games_won_mut(&mut self) -> &mut Le64 {
        &mut self.0.games_won
    }

    /// Mutable handle to the games-finished stat.
    pub fn games_finished_mut(&mut self) -> &mut Le64 {
        &mut self.0.games_finished
    }

    pub fn initial_lock_hash(&self) -> &[u8; 32] {
        &self.0.initial_lock_hash
    }

    pub fn lock_tag(&self) -> u8 {
        self.0.lock_tag
    }

    pub fn lock_body_mut(&mut self) -> &mut [u8] {
        &mut self.0.lock_body
    }
}

/// Total wire length for a user resource carrying `lock`.
pub fn user_total_len(lock: &LockEnum<'_>) -> usize {
    USER_HEADER_LEN + lock.wire_body_len()
}

/// Writes a fresh user wire buffer into `out`. `out` must be pre-sized to
/// `user_total_len(lock)`. The counters are passed explicitly so rewrite
/// paths (lock rotation) preserve them; fresh creation passes zeros.
pub fn write_user(
    out: &mut [u8],
    balance: u64,
    stats: GameStats,
    initial_lock_hash: &[u8; 32],
    lock: &LockEnum<'_>,
) -> Result<(), &'static str> {
    let need = user_total_len(lock);
    if out.len() != need {
        return Err("user: write buffer wrong length");
    }
    out[0] = KIND_USER;
    out[1..9].copy_from_slice(&balance.to_le_bytes());
    out[9..17].copy_from_slice(&stats.started.to_le_bytes());
    out[17..25].copy_from_slice(&stats.won.to_le_bytes());
    out[25..33].copy_from_slice(&stats.finished.to_le_bytes());
    out[33..65].copy_from_slice(initial_lock_hash);
    out[65] = lock.tag();
    lock.write_body(&mut out[USER_HEADER_LEN..]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use vprogs_zk_backend_risc0_runtime_processor::lock_trait::Lock;

    use super::*;
    use crate::runtime::lock::{MultisigLockView, SchnorrLockView, UnlockedLockView};

    /// Offset of the lock-tag byte within the fixed header.
    const LOCK_TAG_OFFSET: usize = core::mem::offset_of!(UserRaw, lock_tag);

    fn pk(b: u8) -> [u8; 32] {
        [b; 32]
    }

    fn hash(b: u8) -> [u8; 32] {
        [b; 32]
    }

    // Schnorr layout

    #[test]
    fn schnorr_round_trip() {
        let pubkey = pk(0x55);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let total = user_total_len(&lock);
        assert_eq!(total, USER_HEADER_LEN + 32);

        let ilh = hash(0xCD);
        let mut buf = vec![0u8; total];
        write_user(&mut buf, 12_345, GameStats { started: 2, won: 1, finished: 2 }, &ilh, &lock)
            .unwrap();

        let view = UserView::from_bytes(&buf).unwrap();
        assert_eq!(view.balance(), 12_345);
        assert_eq!(view.games_started(), 2);
        assert_eq!(view.games_won(), 1);
        assert_eq!(view.games_finished(), 2);
        assert_eq!(view.initial_lock_hash(), &ilh);
        match view.lock() {
            LockEnum::Schnorr(SchnorrLockView { pubkey: pk_back }) => {
                assert_eq!(pk_back, &pubkey);
            }
            _ => panic!("expected Schnorr"),
        }
    }

    #[test]
    fn schnorr_rejects_wrong_length() {
        let buf = vec![0u8; USER_HEADER_LEN + 31];
        // First, set the kind byte so we exercise the body-length check, not the kind check.
        let mut buf = buf;
        buf[0] = KIND_USER;
        buf[LOCK_TAG_OFFSET] = SchnorrLockView::TAG;
        assert!(UserView::from_bytes(&buf).is_err());
    }

    // Multisig layout

    fn build_multisig_body(threshold: u8, pks: &[[u8; 32]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(threshold);
        out.push(pks.len() as u8);
        for p in pks {
            out.extend_from_slice(p);
        }
        out
    }

    #[test]
    fn multisig_round_trip() {
        let pks = [pk(0x01), pk(0x02), pk(0x03)];
        let body = build_multisig_body(2, &pks);
        let lock = LockEnum::Multisig(MultisigLockView { threshold: 2, pubkeys: &body[2..] });
        let total = user_total_len(&lock);
        assert_eq!(total, USER_HEADER_LEN + 2 + 3 * 32);

        let ilh = hash(0xEE);
        let mut buf = vec![0u8; total];
        write_user(&mut buf, 42, GameStats::default(), &ilh, &lock).unwrap();

        let view = UserView::from_bytes(&buf).unwrap();
        assert_eq!(view.balance(), 42);
        assert_eq!(view.initial_lock_hash(), &ilh);
        match view.lock() {
            LockEnum::Multisig(m) => {
                assert_eq!(m.threshold, 2);
                assert_eq!(m.n_pubkeys(), 3);
            }
            _ => panic!("expected Multisig"),
        }
    }

    // Unlocked layout

    #[test]
    fn unlocked_round_trip() {
        let lock = LockEnum::Unlocked(UnlockedLockView);
        let total = user_total_len(&lock);
        assert_eq!(total, USER_HEADER_LEN);

        let ilh = hash(0x77);
        let mut buf = vec![0u8; total];
        write_user(&mut buf, 7, GameStats::default(), &ilh, &lock).unwrap();

        let view = UserView::from_bytes(&buf).unwrap();
        assert_eq!(view.balance(), 7);
        assert_eq!(view.initial_lock_hash(), &ilh);
        assert!(matches!(view.lock(), LockEnum::Unlocked(_)));
    }

    #[test]
    fn rejects_wrong_kind_byte() {
        let pubkey = pk(0x55);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let ilh = hash(0xAA);
        let mut buf = vec![0u8; user_total_len(&lock)];
        write_user(&mut buf, 1, GameStats::default(), &ilh, &lock).unwrap();
        buf[0] = KIND_USER + 7; // bogus
        assert!(UserView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_unknown_lock_tag() {
        let mut buf = vec![0u8; USER_HEADER_LEN];
        buf[0] = KIND_USER;
        buf[LOCK_TAG_OFFSET] = 0xFF;
        assert!(UserView::from_bytes(&buf).is_err());
    }

    // Mutable in-place updates

    #[test]
    fn mutable_view_updates_counters_balance_and_lock_body_in_place() {
        let pubkey = pk(0x11);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let ilh = hash(0x33);
        let mut buf = vec![0u8; user_total_len(&lock)];
        write_user(&mut buf, 100, GameStats { started: 3, won: 2, finished: 3 }, &ilh, &lock)
            .unwrap();

        {
            let mut mv = UserViewMut::from_bytes_mut(&mut buf).unwrap();
            assert_eq!(mv.initial_lock_hash(), &ilh); // permanent
            mv.balance_mut().set(200);
            mv.games_started_mut().set(4);
            mv.games_won_mut().set(3);
            mv.games_finished_mut().set(4);
            mv.lock_body_mut().copy_from_slice(&pk(0x22));
        }

        let view = UserView::from_bytes(&buf).unwrap();
        assert_eq!(view.balance(), 200);
        assert_eq!(view.games_started(), 4);
        assert_eq!(view.games_won(), 3);
        assert_eq!(view.games_finished(), 4);
        assert_eq!(view.initial_lock_hash(), &ilh);
        match view.lock() {
            LockEnum::Schnorr(SchnorrLockView { pubkey }) => assert_eq!(pubkey, &pk(0x22)),
            _ => panic!("expected Schnorr"),
        }
    }
}
