//! This program's deposit policy: the rules an L1-backed deposit obeys.
//!
//! `DepositPolicy` is the seam `apply_deposit` stays generic over: it fixes what L1
//! `script_public_key` a funding output must pay and which user resource a deposit credits,
//! while the apply fn supplies the mechanism (output parsing, dedup, create-vs-credit). The
//! concrete impl is wired in `main.rs`. The types speak this program's [`LockEnum`]; the
//! battery's own deposit-policy module is not used.
//!
//! Deposit-address binding: a funding output must pay
//! `P2SH(delegate_entry_script(config.covenant_id))`, the covenant-spendable delegate-entry
//! script the permission-script sweep recognises. `covenant_id` is sourced from the config
//! resource (committed L2 state, set once at `Init`) rather than baked into the guest, so the
//! zkVM image id is invariant to it.

use vprogs_zk_abi::withdrawal::{ScriptBytes, StandardSpk};
use vprogs_zk_backend_risc0_api::delegate_entry_spk_hash;

use crate::runtime::lock::LockEnum;

/// Minimum funding a new user must be born with, applied uniformly wherever a user resource is
/// created: a `Deposit` whose funding output, or a `Transfer` whose moved amount, falls below
/// this is rejected rather than opening an underfunded account. There is no zero-balance birth.
pub const MIN_CREATE_BALANCE: u64 = 1_000;

/// Decoded deposit action fields, re-exposed so `DepositPolicy` methods can key on them
/// without taking a full `ActionBody` reference.
pub struct DepositBody<'a> {
    /// Resource-list index of the user to credit or create.
    pub user_idx: u8,
    /// Index into the current tx's output list of the funding output.
    pub output_idx: u32,
    /// Initial lock carried by the deposit action. Its `id_hash()` derives the user's resource
    /// address.
    pub initial_lock: LockEnum<'a>,
}

/// Borrowed inputs `deposit_spk` may key off. Kept as a struct (not raw args) so adding context
/// later doesn't churn the trait signature.
pub struct DepositSubject<'a> {
    /// Decoded deposit action fields.
    pub body: &'a DepositBody<'a>,
    /// Config-committed covenant a deposit must pay (read from the config
    /// resource). The deposit address is `P2SH(delegate_entry_script(covenant_id))`.
    pub covenant_id: &'a [u8; 32],
}

/// Resolution of `DepositPolicy::credit_target`.
pub struct CreditTarget {
    /// Resource-list index of the user to credit.
    pub user_idx: u8,
    /// If `true`, the deposit may CREATE the user when the slot is `New`; if `false`, the user
    /// must already exist. A created user always gets the action's `initial_lock` (its
    /// `id_hash()` derives the resource address), so the policy decides only *whether* creation
    /// may happen, never with which lock.
    pub may_create: bool,
}

/// This program's rules a deposit obeys.
///
/// A `DepositPolicy` answers two questions for one deposit action: what L1 `script_public_key`
/// must the funding output pay (via `deposit_spk`), and which user resource does this deposit
/// credit and may it create that user (via `credit_target`).
///
/// No `dyn`: the runtime is monomorphized over the concrete impl chosen in `main.rs`. Methods
/// take `&self` so an impl may carry config (e.g. a treasury key) without globals.
pub trait DepositPolicy {
    /// The on-chain `script_public_key` bytes a deposit's funding output must pay.
    ///
    /// Returned as owned [`ScriptBytes`] (built through a typed `StandardSpk`, so it is one of
    /// the recognised standard scripts and the byte length is fixed by kind). Owned rather than
    /// borrowed because a policy may *derive* the script from `who`.
    fn deposit_spk(&self, who: &DepositSubject<'_>) -> ScriptBytes;

    /// Returns the resource index to credit and whether the action may CREATE that user (vs
    /// credit-existing-only), or `Err` to reject the deposit (surfaced as `AbiError::Decode`).
    ///
    /// The user-resource identity seed is fixed to `initial_lock.id_hash()`. A policy may choose
    /// which user to credit or whether to allow creation, but deriving a non-lock-based identity
    /// requires editing `apply_deposit`, not just this trait.
    fn credit_target(&self, body: &DepositBody<'_>) -> Result<CreditTarget, &'static str>;

    /// Minimum funding a new user must be born with; see [`MIN_CREATE_BALANCE`].
    fn min_create_balance(&self) -> u64;
}

/// This program's deposit policy: a single covenant-bound deposit address shared by all
/// depositors; deposits credit the user named positionally by the action, creating them from
/// the action-carried `initial_lock` when the slot is new.
///
/// Holds no state: the covenant arrives via `DepositSubject::covenant_id` (read from config in
/// `apply_deposit`), so the address is not part of the guest image.
pub struct CovenantDepositPolicy;

impl DepositPolicy for CovenantDepositPolicy {
    fn deposit_spk(&self, who: &DepositSubject<'_>) -> ScriptBytes {
        // The deposit address is the P2SH of the covenant's delegate-entry script, the
        // covenant-spendable script the permission sweep already recognises. A per-user impl
        // would instead derive from `who.body`.
        StandardSpk::ScriptHash(&delegate_entry_spk_hash(who.covenant_id)).to_script_bytes()
    }

    fn credit_target(&self, body: &DepositBody<'_>) -> Result<CreditTarget, &'static str> {
        // Credit the action's `user_idx`; create-or-credit using the carried initial lock
        // (identity == initial_lock_hash, so a new user MUST supply its lock).
        Ok(CreditTarget { user_idx: body.user_idx, may_create: true })
    }

    fn min_create_balance(&self) -> u64 {
        MIN_CREATE_BALANCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::lock::UnlockedLockView;

    /// `deposit_spk` must return the P2SH of the covenant's delegate-entry script, NOT the P2SH
    /// of the raw covenant_id, in the on-chain layout `OpBlake2b | OpData32 | hash(32) | OpEqual`
    /// (35 bytes).
    #[test]
    fn deposit_spk_is_p2sh_of_delegate_entry_script() {
        let policy = CovenantDepositPolicy;
        let covenant_id = [0x44u8; 32];
        let body = DepositBody {
            user_idx: 0,
            output_idx: 0,
            initial_lock: LockEnum::Unlocked(UnlockedLockView),
        };
        let subject = DepositSubject { body: &body, covenant_id: &covenant_id };

        let script_bytes = policy.deposit_spk(&subject);
        let script_slice = script_bytes.as_slice();

        let delegate_hash = delegate_entry_spk_hash(&covenant_id);
        assert_eq!(
            script_slice,
            StandardSpk::ScriptHash(&delegate_hash).to_script_bytes().as_slice()
        );
        assert_ne!(
            script_slice,
            StandardSpk::ScriptHash(&covenant_id).to_script_bytes().as_slice(),
            "deposit address must be P2SH of the delegate script, not of the raw covenant_id",
        );

        assert_eq!(script_slice.len(), 35);
        assert_eq!(script_slice[0], 0xaa); // OpBlake2b
        assert_eq!(script_slice[1], 0x20); // OpData32
        assert_eq!(&script_slice[2..34], &delegate_hash);
        assert_eq!(script_slice[34], 0x87); // OpEqual
    }
}
