//! `Withdraw` action: debit a user and emit an L2-to-L1 exit. The min-withdrawal policy is read
//! from the live config resource.

use vprogs_zk_abi::{Error as AbiError, Result as AbiResult, withdrawal::StandardSpk};

use super::{ApplyContext, view_config_at};
use crate::program::resources::ext::ResourceExt;

/// Debits `amount` from the user at `user_idx` and emits an L2-to-L1 exit to `dest`.
///
/// Authorization uses `LockEnum::unlock` on the user's current lock; any lock variant (Schnorr,
/// Multisig, Unlocked, ...) is accepted without new auth code.
///
/// All reads and authorization checks complete before any state mutation, so a failed check never
/// leaves a partially-applied withdrawal.
pub(super) fn apply_withdraw<'a>(
    user_idx: u8,
    config_idx: u8,
    amount: u64,
    dest: StandardSpk<'a>,
    cx: &mut ApplyContext<'a, '_>,
) -> AbiResult<()> {
    // Read the min-withdrawal policy from the config resource the action names. Config must be
    // present and live to enforce the min.
    let min = view_config_at(cx.resources, config_idx, |c| c.min_withdrawal_amount())?;

    let auth_ok = cx.resources[user_idx as usize]
        .view_user(|v| v.lock().unlock(user_idx, cx.auth_ctx))
        .ok_or_else(|| AbiError::Decode("withdraw: not a live user resource".into()))?;
    if !auth_ok {
        return Err(AbiError::Decode("withdraw: lock not satisfied".into()));
    }

    if amount < min {
        return Err(AbiError::Decode("withdraw: amount below min_withdrawal_amount".into()));
    }

    let cur = cx.resources[user_idx as usize]
        .view_user(|v| v.balance())
        .ok_or_else(|| AbiError::Decode("withdraw: not a live user resource".into()))?;
    let new_balance = cur
        .checked_sub(amount)
        .ok_or_else(|| AbiError::Decode("withdraw: insufficient balance".into()))?;

    cx.resources[user_idx as usize]
        .modify_user(|v| v.balance_mut().set(new_balance))
        .ok_or_else(|| AbiError::Decode("withdraw: not a live user resource".into()))?;

    cx.exits.emit(dest, amount).map_err(|_| AbiError::Decode("withdraw: emit failed".into()))
}
