use vprogs_zk_abi::withdrawal::{ScriptBytes, StandardSpk};
use vprogs_zk_backend_risc0_api::delegate_entry_spk_hash;
use vprogs_zk_backend_risc0_runtime_processor::deposit_policy::{
    CreateSpec, CreditTarget, DepositBody, DepositPolicy, DepositSubject,
};

use crate::runtime::lock::LockEnum;

/// Minimum funding a new user must be born with, applied uniformly wherever a user resource is
/// created: a `Deposit` whose funding output, or a `Transfer` whose moved amount, falls below
/// this is rejected rather than opening an underfunded account. There is no zero-balance birth.
pub const MIN_CREATE_BALANCE: u64 = 1_000;

/// This program's deposit policy
pub struct CovenantDepositPolicy;

impl DepositPolicy for CovenantDepositPolicy {
    type Lock<'a> = LockEnum<'a>;

    fn deposit_spk<'a>(&self, who: &DepositSubject<'_, Self::Lock<'a>>) -> ScriptBytes {
        // The deposit address is the P2SH of the covenant's delegate-entry script, the
        // covenant-spendable script the permission sweep already recognises. A per-user impl
        // would instead derive from `who.body`.
        StandardSpk::ScriptHash(&delegate_entry_spk_hash(who.covenant_id)).to_script_bytes()
    }

    fn credit_target<'a>(
        &self,
        body: &DepositBody<Self::Lock<'a>>,
    ) -> Result<CreditTarget<Self::Lock<'a>>, &'static str> {
        // Credit the action's `user_idx`; create-or-credit using the carried initial lock
        // (identity == initial_lock_hash, so a new user MUST supply its lock).
        Ok(CreditTarget {
            user_idx: body.user_idx,
            create_with: Some(CreateSpec { initial_lock: body.initial_lock }),
        })
    }

    fn min_create_balance(&self) -> u64 {
        MIN_CREATE_BALANCE
    }
}

#[cfg(test)]
mod tests {
    use vprogs_zk_backend_risc0_runtime_processor::deposit_policy::{DepositBody, DepositSubject};

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
