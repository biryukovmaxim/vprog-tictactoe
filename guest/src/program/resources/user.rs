//! User resource (`derive_user_resource(initial_lock_hash)`).
//!
//! Wire layout: kind byte + fixed header + tag-driven variable body:
//! ```text
//! [0]       kind                (Kind::User = 1; see `crate::program::resources::kind`)
//! [1..9]    balance             (u64 LE)
//! [9..17]   games_started       (u64 LE; game-id derivation seed)
//! [17..25]  games_won           (u64 LE)
//! [25..33]  games_finished      (u64 LE)
//! [33..65]  initial_lock_hash   ([u8; 32])
//! [65]      lock_tag            (current lock, may differ from initial)
//! [66..]    lock_body           (length and shape implied by tag)
//! ```
//!
//! The kind byte is checked by `from_bytes` before the kindless body is
//! zerocopy-parsed, so it carries no field of [`UserView`] itself.

use zerocopy::{
    FromZeros, Immutable, IntoBytes, KnownLayout, TryFromBytes, Unaligned,
    little_endian::U64 as Le64,
};

use crate::{
    program::resources::kind::{Kind, kind_of},
    runtime::{
        lock::LockEnum,
        lock_codec::{decode_lock_body_unchecked, validate_lock_body},
    },
};

/// Fixed-header byte length: `kind (u8) || balance (u64 LE) || games_started (u64 LE) ||
/// games_won (u64 LE) || games_finished (u64 LE) || initial_lock_hash ([u8; 32]) || lock_tag (u8)`,
/// derived from the struct layout so the sum can never drift.
pub const USER_HEADER_LEN: usize = core::mem::offset_of!(UserView, lock_tag) + 2;

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

/// Zerocopy DST over the kindless body: fixed header + tag-driven variable tail.
/// Fields are private; a handle is obtainable only through the validating
/// `from_bytes` / `from_bytes_mut`, so accessors are infallible.
#[repr(C)]
#[derive(FromZeros, IntoBytes, Immutable, KnownLayout, Unaligned)]
pub struct UserView {
    balance: Le64,
    games_started: Le64,
    games_won: Le64,
    games_finished: Le64,
    initial_lock_hash: [u8; 32],
    lock_tag: u8,
    lock_body: [u8],
}

impl UserView {
    /// Validates `bytes` (kind byte + body) and returns the read view over them.
    pub fn from_bytes(bytes: &[u8]) -> Result<&Self, &'static str> {
        if kind_of(bytes) != Some(Kind::User) {
            return Err("user: wrong kind");
        }
        let v = Self::try_ref_from_bytes(&bytes[1..]).map_err(|_| "user: invalid layout")?;
        validate_lock_body(v.lock_tag, &v.lock_body)?;
        Ok(v)
    }

    /// Mutable counterpart of [`Self::from_bytes`] for fixed-field updates. The
    /// lock body can be rewritten via `lock_body_mut`; the caller is responsible
    /// for keeping tag-implied invariants intact. `initial_lock_hash` is *not*
    /// exposed mutably; it's permanent.
    pub fn from_bytes_mut(bytes: &mut [u8]) -> Result<&mut Self, &'static str> {
        if kind_of(bytes) != Some(Kind::User) {
            return Err("user: wrong kind");
        }
        let v = Self::try_mut_from_bytes(&mut bytes[1..]).map_err(|_| "user: invalid layout")?;
        validate_lock_body(v.lock_tag, &v.lock_body)?;
        Ok(v)
    }

    /// Spendable balance.
    pub fn balance(&self) -> u64 {
        self.balance.get()
    }

    /// Games this player ever started; seeds the game-resource id derivation.
    pub fn games_started(&self) -> u64 {
        self.games_started.get()
    }

    /// Games this player has won (settled in their favor).
    pub fn games_won(&self) -> u64 {
        self.games_won.get()
    }

    /// Games this player has finished (any outcome).
    pub fn games_finished(&self) -> u64 {
        self.games_finished.get()
    }

    /// The game counters as one value.
    pub fn stats(&self) -> GameStats {
        GameStats {
            started: self.games_started.get(),
            won: self.games_won.get(),
            finished: self.games_finished.get(),
        }
    }

    /// Identity hash of the lock the account was born with; permanent.
    pub fn initial_lock_hash(&self) -> &[u8; 32] {
        &self.initial_lock_hash
    }

    /// Tag of the lock variant whose body follows the header.
    pub fn lock_tag(&self) -> u8 {
        self.lock_tag
    }

    /// Returns the typed lock view over the (current) body bytes.
    ///
    /// Infallible by construction: `from_bytes` validated this (tag, body) pair.
    pub fn lock(&self) -> LockEnum<'_> {
        decode_lock_body_unchecked(self.lock_tag, &self.lock_body)
    }

    /// Mutable handle to the on-disk balance. The returned `Le64` already
    /// owns the get/set API (`.get() -> u64`, `.set(u64)`); combining read
    /// and write through one borrow avoids the get-then-set ceremony.
    pub fn balance_mut(&mut self) -> &mut Le64 {
        &mut self.balance
    }

    /// Mutable handle to the games-started counter (game-id derivation seed).
    pub fn games_started_mut(&mut self) -> &mut Le64 {
        &mut self.games_started
    }

    /// Mutable handle to the games-won stat.
    pub fn games_won_mut(&mut self) -> &mut Le64 {
        &mut self.games_won
    }

    /// Mutable handle to the games-finished stat.
    pub fn games_finished_mut(&mut self) -> &mut Le64 {
        &mut self.games_finished
    }

    /// The lock body bytes, mutable for same-shape rewrites.
    pub fn lock_body_mut(&mut self) -> &mut [u8] {
        &mut self.lock_body
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
    out[0] = Kind::User as u8;
    let v = UserView::try_mut_from_bytes(&mut out[1..]).map_err(|_| "user: invalid layout")?;
    v.balance = Le64::new(balance);
    v.games_started = Le64::new(stats.started);
    v.games_won = Le64::new(stats.won);
    v.games_finished = Le64::new(stats.finished);
    v.initial_lock_hash = *initial_lock_hash;
    v.lock_tag = lock.tag();
    lock.write_body(&mut v.lock_body);
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use vprogs_zk_backend_risc0_runtime_processor::lock_trait::Lock;

    use super::*;
    use crate::runtime::lock::{MultisigLockView, SchnorrLockView, UnlockedLockView};

    /// Offset of the lock-tag byte within the wire buffer.
    const LOCK_TAG_OFFSET: usize = USER_HEADER_LEN - 1;

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
        let mut buf = vec![0u8; USER_HEADER_LEN + 31];
        // First, set the kind byte so we exercise the body-length check, not the kind check.
        buf[0] = Kind::User as u8;
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
        buf[0] = Kind::User as u8 + 7; // bogus
        assert!(UserView::from_bytes(&buf).is_err());
    }

    /// Kind 0 is the config kind; it must not parse as a user even though the old
    /// `Unset` construction value sat there.
    #[test]
    fn rejects_config_kind_byte() {
        let pubkey = pk(0x55);
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let mut buf = vec![0u8; user_total_len(&lock)];
        write_user(&mut buf, 1, GameStats::default(), &hash(0xAA), &lock).unwrap();
        buf[0] = Kind::Config as u8;
        assert!(UserView::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_unknown_lock_tag() {
        let mut buf = vec![0u8; USER_HEADER_LEN];
        buf[0] = Kind::User as u8;
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
            let mv = UserView::from_bytes_mut(&mut buf).unwrap();
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
