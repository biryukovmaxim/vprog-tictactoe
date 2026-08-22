mod config;
mod deposit;
mod user;
mod withdraw;

use alloc::vec::Vec;

use config::{apply_init, apply_update};
use deposit::apply_deposit;
use user::{apply_transfer, apply_update_user_lock};
use vprogs_core_codec::{Error, Reader, Result as CodecResult};
use vprogs_core_types::ResourceId;
use vprogs_zk_abi::{
    Error as AbiError, Result as AbiResult,
    transaction_processor::{Resource, Transaction},
    withdrawal::{DepositSink, ExitSink, StandardSpk},
};
use vprogs_zk_backend_risc0_runtime_processor::{auth_context::AuthContext, lifecycle::Lifecycle};
use withdraw::apply_withdraw;

use crate::{
    program::{
        config::ConfigView,
        deposit_policy::DepositPolicy,
        resource_ext::ResourceExt,
        resource_id::{config_resource_id, derive_user_resource},
    },
    runtime::{
        ix::read_resource_idx,
        lock::{LockEnum, decode_lock},
    },
};

/// Action variant: update an existing config resource.
pub const ACTION_TAG_UPDATE: u8 = 0x01;
/// Action variant: bootstrap (create) the singleton config resource. Gated by
/// the env-provided genesis pubkey at apply time.
pub const ACTION_TAG_INIT: u8 = 0x02;
/// Action variant: move balance between two user resources, creating the
/// destination when its slot is new and a dest lock is supplied. Auth is checked
/// against the source's current lock; the destination is not authed.
pub const ACTION_TAG_TRANSFER: u8 = 0x03;
/// Action variant: rotate the lock on a user resource. The current lock must
/// authorize the rotation; `initial_lock_hash` is preserved.
pub const ACTION_TAG_UPDATE_USER_LOCK: u8 = 0x04;
/// Action variant: credit a user from an L1 deposit output, creating the user
/// when the slot is new (the sole path that creates a user resource). The
/// funding output at `output_idx` of this tx must pay
/// `DepositPolicy::deposit_spk(..)`; the credited amount is that output's
/// `value`.
pub const ACTION_TAG_DEPOSIT: u8 = 0x05;
/// Action variant: debit a user and emit an L2-to-L1 exit to `dest`. Authorized by the user's
/// current lock; enforces `config.min_withdrawal_amount`.
pub const ACTION_TAG_WITHDRAW: u8 = 0x06;
/// Action variant: create an open staked game. Auth: creator's user lock. (Lands with the game
/// milestone; tag reserved.)
pub const ACTION_TAG_CREATE_GAME: u8 = 0x07;
/// Action variant: join an open game, locking the matching stake. Auth: joiner's user lock.
/// (Reserved.)
pub const ACTION_TAG_JOIN_GAME: u8 = 0x08;
/// Action variant: place a mark on the board of a playing game. Auth: the to-move player's lock.
/// (Reserved.)
pub const ACTION_TAG_TURN: u8 = 0x09;

/// Read view over a single action entry.
pub struct ActionView<'a> {
    pub action_tag: u8,
    pub body: ActionBody<'a>,
}

pub enum ActionBody<'a> {
    Update {
        config_idx: u8,
        new_min_withdrawal_amount: u64,
        /// Carried for wire-shape symmetry with `Init`. `apply_update` rejects
        /// any change here: covenant_id is immutable after `Init`.
        new_covenant_id: [u8; 32],
        new_lock: LockEnum<'a>,
    },
    Init {
        config_idx: u8,
        new_min_withdrawal_amount: u64,
        /// The covenant a deposit's funding output must pay (as P2SH of its
        /// delegate-entry script). Written into config state once at `Init`;
        /// immutable thereafter.
        new_covenant_id: [u8; 32],
        new_lock: LockEnum<'a>,
    },
    Transfer {
        source_idx: u8,
        dest_idx: u8,
        amount: u64,
        /// Lock for the destination, present only to CREATE a new dest slot from this transfer
        /// (its `id_hash()` derives the new user's address). `None` credits an existing
        /// destination.
        dest_init: Option<LockEnum<'a>>,
    },
    UpdateUserLock {
        user_idx: u8,
        new_lock: LockEnum<'a>,
    },
    Deposit {
        /// Resource-list index of the user resource credited, or created when the slot is new.
        user_idx: u8,
        /// Resource-list index of the config resource the deposit's covenant binding is read
        /// from. Must name the singleton config resource.
        config_idx: u8,
        /// Index into the current tx's output list of the funding output whose
        /// `value` is credited and whose SPK must match the deposit policy.
        output_idx: u32,
        /// The user's initial lock. Its `id_hash()` derives the user address;
        /// on a CREATE it becomes the new user's lock, on a credit it must
        /// match the existing `initial_lock_hash` (address binding).
        initial_lock: LockEnum<'a>,
    },
    Withdraw {
        /// Resource-list index of the user being debited.
        user_idx: u8,
        /// Resource-list index of the config resource `min_withdrawal_amount` is read from.
        /// Must name the singleton config resource.
        config_idx: u8,
        /// Amount to withdraw; debited from the user and emitted as the exit value.
        amount: u64,
        /// Typed L1 destination for the emitted exit. Length-by-tag prevents the byte-length
        /// foot-gun of a raw script slice.
        dest: StandardSpk<'a>,
    },
}

/// The program's action decoder, handed to `runtime::ix::decode_ix`.
/// Decodes one action entry (`action_tag u8 || body`), bounds-checking every
/// resource index against `n_resources`.
pub fn decode_action<'a>(buf: &mut &'a [u8], n_resources: usize) -> CodecResult<ActionView<'a>> {
    let action_tag = buf.byte("action.action_tag")?;
    let body = match action_tag {
        ACTION_TAG_UPDATE => {
            let config_idx = read_resource_idx(buf, "action.update.config_idx", n_resources)?;
            let new_min_withdrawal_amount =
                buf.le_u64("action.update.new_min_withdrawal_amount")?;
            let new_covenant_id = *buf.array::<32>("action.update.new_covenant_id")?;
            let new_lock = decode_lock(buf)?;
            ActionBody::Update { config_idx, new_min_withdrawal_amount, new_covenant_id, new_lock }
        }
        ACTION_TAG_INIT => {
            let config_idx = read_resource_idx(buf, "action.init.config_idx", n_resources)?;
            let new_min_withdrawal_amount = buf.le_u64("action.init.new_min_withdrawal_amount")?;
            let new_covenant_id = *buf.array::<32>("action.init.new_covenant_id")?;
            let new_lock = decode_lock(buf)?;
            ActionBody::Init { config_idx, new_min_withdrawal_amount, new_covenant_id, new_lock }
        }
        ACTION_TAG_TRANSFER => {
            let source_idx = read_resource_idx(buf, "action.transfer.source_idx", n_resources)?;
            let dest_idx = read_resource_idx(buf, "action.transfer.dest_idx", n_resources)?;
            let amount = buf.le_u64("action.transfer.amount")?;
            // One presence byte gates an optional dest lock: 0 = credit existing, 1 = lock follows
            // (create-if-new). Any other value is malformed.
            let dest_init = match buf.byte("action.transfer.has_dest_lock")? {
                0 => None,
                1 => Some(decode_lock(buf)?),
                _ => return Err(Error::Decode("action.transfer: bad has_dest_lock flag")),
            };
            ActionBody::Transfer { source_idx, dest_idx, amount, dest_init }
        }
        ACTION_TAG_UPDATE_USER_LOCK => {
            let user_idx = read_resource_idx(buf, "action.update_user_lock.user_idx", n_resources)?;
            let new_lock = decode_lock(buf)?;
            ActionBody::UpdateUserLock { user_idx, new_lock }
        }
        ACTION_TAG_DEPOSIT => {
            let user_idx = read_resource_idx(buf, "action.deposit.user_idx", n_resources)?;
            let config_idx = read_resource_idx(buf, "action.deposit.config_idx", n_resources)?;
            let output_idx = buf.le_u32("action.deposit.output_idx")?;
            let initial_lock = decode_lock(buf)?;
            ActionBody::Deposit { user_idx, config_idx, output_idx, initial_lock }
        }
        ACTION_TAG_WITHDRAW => {
            let user_idx = read_resource_idx(buf, "action.withdraw.user_idx", n_resources)?;
            let config_idx = read_resource_idx(buf, "action.withdraw.config_idx", n_resources)?;
            let amount = buf.le_u64("action.withdraw.amount")?;
            // StandardSpk::decode returns vprogs_zk_abi::Result; map into
            // the CodecResult this function returns.
            let dest = StandardSpk::decode(buf)
                .map_err(|_| Error::Decode("action.withdraw: bad dest spk"))?;
            ActionBody::Withdraw { user_idx, config_idx, amount, dest }
        }
        _ => return Err(Error::Decode("action: unknown tag")),
    };
    Ok(ActionView { action_tag, body })
}

/// Everything an apply fn may need, bundled once so dispatch and apply signatures stay stable as
/// capabilities grow.
///
/// Plain typed fields, passed by `&mut`; no dynamic dispatch, no extractor magic.
///
/// `'a` is the transaction/resource data lifetime; every buffer borrow lives here. `'cx` is the
/// shorter borrow lifetime for the mutable references (`resources`, `exits`, `deposit`) and for
/// `auth_ctx`, which is computed inside `run` and does not outlive it. The two lifetimes are
/// independent: `'cx: 'a` is not required.
pub struct ApplyContext<'a, 'cx> {
    /// Decoded transaction; its `rest_preimage` is the L1 source of truth for deposit output
    /// values.
    pub tx: &'cx Transaction<'a>,
    /// Resource set addressed positionally by action indices.
    pub resources: &'cx mut [Resource<'a>],
    /// Per-resource lifecycle state, parallel to `resources`, advanced as actions apply within
    /// this tx so create-vs-credit is decided from the live state rather than the input
    /// snapshot.
    pub lifecycle: Vec<Lifecycle>,
    /// Resolved signer authority, consulted via `LockEnum::unlock`.
    pub auth_ctx: &'cx AuthContext,
    /// L2-to-L1 exit accumulator.
    pub exits: &'cx mut ExitSink,
    /// Deposit-address commitment sink, written by `apply_deposit`.
    pub deposit: &'cx mut DepositSink,
    /// Output indices consumed by `Deposit` actions in this tx; prevents double-credit within one
    /// tx.
    pub consumed_outputs: Vec<u32>,
}

impl<'a, 'cx> ApplyContext<'a, 'cx> {
    /// Builds the context, seeding each resource's starting lifecycle from its decoded snapshot.
    pub fn new(
        tx: &'cx Transaction<'a>,
        resources: &'cx mut [Resource<'a>],
        auth_ctx: &'cx AuthContext,
        exits: &'cx mut ExitSink,
        deposit: &'cx mut DepositSink,
    ) -> Self {
        let lifecycle = resources.iter().map(Lifecycle::from_resource).collect();
        Self { tx, resources, lifecycle, auth_ctx, exits, deposit, consumed_outputs: Vec::new() }
    }

    /// Current lifecycle state of the resource at `idx`.
    pub fn lifecycle(&self, idx: usize) -> Lifecycle {
        self.lifecycle[idx]
    }

    /// Drives the `New -> Live` create transition for `idx`, rejecting double-create and
    /// re-create-after-delete. The caller writes the new payload separately.
    pub fn mark_created(&mut self, idx: usize) -> Result<(), &'static str> {
        self.lifecycle[idx] = self.lifecycle[idx].created()?;
        Ok(())
    }

    /// Drives the `Live -> Deleted` delete transition for `idx` and empties the slot's data so the
    /// journal commits its teardown (`EMPTY_HASH`). Rejects deleting a never-created or
    /// already-deleted slot.
    pub fn mark_deleted(&mut self, idx: usize) -> Result<(), &'static str> {
        self.lifecycle[idx] = self.lifecycle[idx].deleted()?;
        self.resources[idx].resize(0);
        Ok(())
    }
}

/// Applies a single decoded action against the context. Generic over the deposit policy `P`; all
/// non-deposit arms ignore it.
pub fn apply_action<'a, P: DepositPolicy>(
    action: &ActionView<'a>,
    cx: &mut ApplyContext<'a, '_>,
    policy: &P,
) -> AbiResult<()> {
    match &action.body {
        ActionBody::Update {
            config_idx: updater_idx,
            new_min_withdrawal_amount,
            new_covenant_id,
            new_lock,
        } => apply_update(*updater_idx, *new_min_withdrawal_amount, new_covenant_id, new_lock, cx),
        ActionBody::Init {
            config_idx: updater_idx,
            new_min_withdrawal_amount,
            new_covenant_id,
            new_lock,
        } => apply_init(*updater_idx, *new_min_withdrawal_amount, new_covenant_id, new_lock, cx),
        ActionBody::Transfer { source_idx, dest_idx, amount, dest_init } => {
            apply_transfer(*source_idx, *dest_idx, *amount, dest_init, cx, policy)
        }
        ActionBody::UpdateUserLock { user_idx, new_lock } => {
            apply_update_user_lock(*user_idx, new_lock, cx)
        }
        ActionBody::Deposit { user_idx, config_idx, output_idx, initial_lock } => {
            apply_deposit(*user_idx, *config_idx, *output_idx, initial_lock, cx, policy)
        }
        ActionBody::Withdraw { user_idx, config_idx, amount, dest } => {
            apply_withdraw(*user_idx, *config_idx, *amount, *dest, cx)
        }
    }
}

/// Reads a config field via the resource at `config_idx`, shared by the actions that carry an
/// explicit config index (`Deposit`, `Withdraw`) instead of scanning the resource list.
///
/// The index must name the singleton config resource: the id check preserves the binding the
/// scan performed, and `view_config`'s `None` (wrong kind, malformed, or emptied slot) rejects
/// a not-live config. The decoder has already bounds-checked the index against `n_resources`.
pub(super) fn view_config_at<R>(
    resources: &[Resource<'_>],
    config_idx: u8,
    f: impl FnOnce(ConfigView<'_>) -> R,
) -> AbiResult<R> {
    let r = &resources[config_idx as usize];
    if r.id() != &config_resource_id() {
        return Err(AbiError::Decode("config_idx does not name the config resource".into()));
    }
    r.view_config(f).ok_or_else(|| AbiError::Decode("config resource not live".into()))
}

/// Validates that a new user slot may be opened at `initial_lock`'s derived address with `funding`,
/// and returns the `initial_lock_hash` for the caller's subsequent `init_user`.
///
/// Shared by every create path (`Deposit` and `Transfer`) so the address binding and the policy
/// creation minimum stay identical wherever a user is born. Pure validation: the caller decides to
/// create from the slot's live lifecycle state ([`ApplyContext::lifecycle`]) and drives the
/// `New -> Live` transition via [`ApplyContext::mark_created`] (which rejects double-create),
/// keeping the caller's "all checks before any state change" ordering.
pub(super) fn validate_user_create(
    slot_id: &ResourceId,
    initial_lock: &LockEnum<'_>,
    funding: u64,
    min_balance: u64,
) -> AbiResult<[u8; 32]> {
    let ilh = initial_lock.id_hash();
    if slot_id != &derive_user_resource(&ilh) {
        return Err(AbiError::Decode(
            "create: slot id != derive_user_resource(initial_lock)".into(),
        ));
    }
    if funding < min_balance {
        return Err(AbiError::Decode("create: funding below policy minimum to create user".into()));
    }
    Ok(ilh)
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use vprogs_zk_backend_risc0_runtime_processor::lock_trait::Lock;

    use super::*;
    use crate::runtime::{ix::decode_ix, lock::SchnorrLockView};

    // Every framing-level ix test (`decode_ix` + signers + tail) lives in
    // `runtime::ix`; the tests here exercise the program's action decoder.

    /// Builds an `Update` action body:
    /// `updater_idx u8 || new_min u64 || covenant_id[32] || schnorr_lock(pk)`.
    fn update_action(updater_idx: u8, new_min: u64, pk: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_UPDATE);
        body.push(updater_idx);
        body.extend_from_slice(&new_min.to_le_bytes());
        body.extend_from_slice(&[0xD7u8; 32]); // covenant_id
        body.push(SchnorrLockView::TAG);
        body.extend_from_slice(&pk);
        body
    }

    #[test]
    fn decode_action_update_with_schnorr_lock() {
        // ix = empty signers || one Update action with Schnorr lock || empty tail
        let mut ix = 0u32.to_le_bytes().to_vec(); // n_signers = 0
        ix.extend_from_slice(&1u32.to_le_bytes()); // n_actions = 1
        ix.extend_from_slice(&update_action(0, 12345, [0xAAu8; 32]));

        let decoded = decode_ix(&ix, 1, decode_action).unwrap();
        assert!(decoded.signers.is_empty());
        assert_eq!(decoded.actions.len(), 1);
        match &decoded.actions[0].body {
            ActionBody::Update {
                config_idx: updater_idx,
                new_min_withdrawal_amount,
                new_covenant_id,
                new_lock,
            } => {
                assert_eq!(*updater_idx, 0);
                assert_eq!(*new_min_withdrawal_amount, 12345);
                assert_eq!(new_covenant_id, &[0xD7u8; 32]);
                assert_eq!(new_lock.tag(), SchnorrLockView::TAG);
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn decode_action_rejects_updater_idx_out_of_range() {
        // updater_idx = 2, but only 2 resources declared (valid range: 0..=1).
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&update_action(2, 12345, [0xAAu8; 32]));

        assert!(decode_ix(&ix, 2, decode_action).is_err());
    }

    #[test]
    fn decode_action_accepts_updater_idx_at_upper_bound() {
        // updater_idx = 1 with n_resources = 2 is valid.
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&update_action(1, 12345, [0xAAu8; 32]));

        let decoded = decode_ix(&ix, 2, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Update { config_idx: updater_idx, .. } => assert_eq!(*updater_idx, 1),
            _ => panic!("expected Update"),
        }
    }

    /// With three resources declared, every in-range `updater_idx` is accepted.
    /// This is the multi-resource analogue of the single-resource happy path:
    /// the program addresses one of N resources by its position in the
    /// id-sorted access metadata. The decoder is order-agnostic; it only
    /// enforces `updater_idx < n_resources`; mapping idx → semantic resource
    /// is the encoder's job.
    #[test]
    fn decode_action_accepts_each_idx_in_three_resource_set() {
        for idx in 0..3u8 {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&update_action(idx, 999, [0xBBu8; 32]));

            let decoded = decode_ix(&ix, 3, decode_action).unwrap();
            match &decoded.actions[0].body {
                ActionBody::Update { config_idx: updater_idx, .. } => {
                    assert_eq!(*updater_idx, idx, "round-trip must preserve idx");
                }
                _ => panic!("expected Update"),
            }
        }
    }

    /// Two actions in one ix can target distinct resources by index, e.g.
    /// updater_idx=0 then updater_idx=2 in a 3-resource set. Mirrors a tx
    /// where the program operates on resources whose id-sorted positions are
    /// non-contiguous.
    #[test]
    fn decode_two_actions_target_different_resources() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&2u32.to_le_bytes()); // n_actions = 2
        ix.extend_from_slice(&update_action(0, 100, [0x11u8; 32]));
        ix.extend_from_slice(&update_action(2, 200, [0x22u8; 32]));

        let decoded = decode_ix(&ix, 3, decode_action).unwrap();
        assert_eq!(decoded.actions.len(), 2);
        let idx0 = match &decoded.actions[0].body {
            ActionBody::Update { config_idx: updater_idx, .. } => *updater_idx,
            _ => panic!("expected Update"),
        };
        let idx1 = match &decoded.actions[1].body {
            ActionBody::Update { config_idx: updater_idx, .. } => *updater_idx,
            _ => panic!("expected Update"),
        };
        assert_eq!((idx0, idx1), (0, 2));
    }

    /// One bad index in a multi-action stream rejects the whole ix; there's
    /// no partial decode. Validates that bounds-checking happens inline as
    /// each action is parsed, not as a post-pass.
    #[test]
    fn decode_rejects_when_one_action_idx_out_of_range() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&2u32.to_le_bytes());
        ix.extend_from_slice(&update_action(1, 100, [0x33u8; 32])); // valid
        ix.extend_from_slice(&update_action(5, 200, [0x44u8; 32])); // out of range

        assert!(decode_ix(&ix, 3, decode_action).is_err());
    }

    /// The decoder doesn't care about the *order* in which actions reference
    /// resources, only that each idx is in range. A descending (or any
    /// permuted) sequence of indices is fine. The same wire format admits all
    /// ordering choices the encoder needs to make.
    #[test]
    fn decode_accepts_descending_action_idxs() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&3u32.to_le_bytes());
        ix.extend_from_slice(&update_action(2, 1, [0x55u8; 32]));
        ix.extend_from_slice(&update_action(1, 2, [0x66u8; 32]));
        ix.extend_from_slice(&update_action(0, 3, [0x77u8; 32]));

        let decoded = decode_ix(&ix, 3, decode_action).unwrap();
        let idxs: Vec<u8> = decoded
            .actions
            .iter()
            .map(|a| match &a.body {
                ActionBody::Update { config_idx: updater_idx, .. } => *updater_idx,
                _ => panic!("expected Update"),
            })
            .collect();
        assert_eq!(idxs, vec![2, 1, 0]);
    }

    // Transfer / UpdateUserLock decoder arms

    fn transfer_action(source: u8, dest: u8, amount: u64) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_TRANSFER);
        body.push(source);
        body.push(dest);
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(0); // has_dest_lock = 0 (credit existing)
        body
    }

    fn transfer_create_action(source: u8, dest: u8, amount: u64, pk: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_TRANSFER);
        body.push(source);
        body.push(dest);
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(1); // has_dest_lock = 1 (create-if-new)
        body.push(SchnorrLockView::TAG);
        body.extend_from_slice(&pk);
        body
    }

    fn update_user_lock_action(user_idx: u8, pk: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_UPDATE_USER_LOCK);
        body.push(user_idx);
        body.push(SchnorrLockView::TAG);
        body.extend_from_slice(&pk);
        body
    }

    #[test]
    fn decode_transfer_action() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&transfer_action(0, 1, 500));

        let decoded = decode_ix(&ix, 2, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Transfer { source_idx, dest_idx, amount, dest_init } => {
                assert_eq!(*source_idx, 0);
                assert_eq!(*dest_idx, 1);
                assert_eq!(*amount, 500);
                assert!(dest_init.is_none());
            }
            _ => panic!("expected Transfer"),
        }
    }

    #[test]
    fn decode_transfer_create_action_carries_dest_lock() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&transfer_create_action(0, 1, 500, [0xEEu8; 32]));

        let decoded = decode_ix(&ix, 2, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Transfer { dest_init, .. } => {
                let lock = dest_init.as_ref().expect("dest lock present");
                assert_eq!(lock.tag(), SchnorrLockView::TAG);
            }
            _ => panic!("expected Transfer"),
        }
    }

    #[test]
    fn decode_transfer_rejects_bad_has_dest_lock_flag() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        let mut body = Vec::new();
        body.push(ACTION_TAG_TRANSFER);
        body.push(0); // source
        body.push(1); // dest
        body.extend_from_slice(&500u64.to_le_bytes());
        body.push(2); // invalid presence flag
        ix.extend_from_slice(&body);

        assert!(decode_ix(&ix, 2, decode_action).is_err());
    }

    #[test]
    fn decode_transfer_rejects_out_of_range_index() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        // dest_idx = 5 but only 2 resources declared
        ix.extend_from_slice(&transfer_action(0, 5, 500));

        assert!(decode_ix(&ix, 2, decode_action).is_err());
    }

    #[test]
    fn decode_update_user_lock_action() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&update_user_lock_action(0, [0xBBu8; 32]));

        let decoded = decode_ix(&ix, 1, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::UpdateUserLock { user_idx, new_lock } => {
                assert_eq!(*user_idx, 0);
                assert_eq!(new_lock.tag(), SchnorrLockView::TAG);
            }
            _ => panic!("expected UpdateUserLock"),
        }
    }

    // Deposit / Withdraw decoder arms

    /// Builds a Deposit action: `tag | user_idx | config_idx | output_idx(4 LE) |
    /// schnorr_lock(pk)`.
    fn deposit_action(user_idx: u8, config_idx: u8, output_idx: u32, pk: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_DEPOSIT);
        body.push(user_idx);
        body.push(config_idx);
        body.extend_from_slice(&output_idx.to_le_bytes());
        body.push(SchnorrLockView::TAG);
        body.extend_from_slice(&pk);
        body
    }

    /// Builds a Withdraw action: `tag | user_idx | config_idx | amount(8 LE) | spk_tag |
    /// spk_payload`.
    fn withdraw_action_pubkey(user_idx: u8, amount: u64, pk: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_WITHDRAW);
        body.push(user_idx);
        body.push(0); // config_idx
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(0x00); // StandardSpk::PubKey tag
        body.extend_from_slice(&pk);
        body
    }

    fn withdraw_action_pubkey_ecdsa(user_idx: u8, amount: u64, pk: [u8; 33]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_WITHDRAW);
        body.push(user_idx);
        body.push(0); // config_idx
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(0x01); // StandardSpk::PubKeyEcdsa tag
        body.extend_from_slice(&pk);
        body
    }

    fn withdraw_action_script_hash(user_idx: u8, amount: u64, hash: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ACTION_TAG_WITHDRAW);
        body.push(user_idx);
        body.push(0); // config_idx
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(0x08); // StandardSpk::ScriptHash tag
        body.extend_from_slice(&hash);
        body
    }

    #[test]
    fn decode_deposit_action() {
        // Distinct user_idx / config_idx values pin the byte order of the two indices.
        let pk = [0xDDu8; 32];
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&deposit_action(0, 1, 3, pk));

        let decoded = decode_ix(&ix, 2, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Deposit { user_idx, config_idx, output_idx, initial_lock } => {
                assert_eq!(*user_idx, 0);
                assert_eq!(*config_idx, 1);
                assert_eq!(*output_idx, 3);
                assert_eq!(initial_lock.tag(), SchnorrLockView::TAG);
            }
            _ => panic!("expected Deposit"),
        }
    }

    #[test]
    fn decode_withdraw_action_pubkey() {
        let pk = [0xAAu8; 32];
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&withdraw_action_pubkey(0, 1_000, pk));

        let decoded = decode_ix(&ix, 1, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Withdraw { user_idx, config_idx, amount, dest } => {
                assert_eq!(*user_idx, 0);
                assert_eq!(*config_idx, 0);
                assert_eq!(*amount, 1_000);
                use vprogs_zk_abi::withdrawal::StandardSpk;
                assert_eq!(*dest, StandardSpk::PubKey(&pk));
            }
            _ => panic!("expected Withdraw"),
        }
    }

    #[test]
    fn decode_withdraw_action_pubkey_ecdsa() {
        let pk = [0xBBu8; 33];
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&withdraw_action_pubkey_ecdsa(0, 500, pk));

        let decoded = decode_ix(&ix, 1, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Withdraw { dest, .. } => {
                use vprogs_zk_abi::withdrawal::StandardSpk;
                assert_eq!(*dest, StandardSpk::PubKeyEcdsa(&pk));
            }
            _ => panic!("expected Withdraw"),
        }
    }

    #[test]
    fn decode_withdraw_action_script_hash() {
        let hash = [0xCCu8; 32];
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&withdraw_action_script_hash(0, 9_999, hash));

        let decoded = decode_ix(&ix, 1, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Withdraw { dest, amount, .. } => {
                use vprogs_zk_abi::withdrawal::StandardSpk;
                assert_eq!(*dest, StandardSpk::ScriptHash(&hash));
                assert_eq!(*amount, 9_999);
            }
            _ => panic!("expected Withdraw"),
        }
    }

    #[test]
    fn decode_deposit_rejects_out_of_range_user_idx() {
        // n_resources = 1, so user_idx = 1 is out of range.
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&deposit_action(1, 0, 0, [0xAAu8; 32]));

        assert!(decode_ix(&ix, 1, decode_action).is_err());
    }

    #[test]
    fn decode_deposit_rejects_out_of_range_config_idx() {
        // n_resources = 1, so config_idx = 1 is out of range.
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&deposit_action(0, 1, 0, [0xAAu8; 32]));

        assert!(decode_ix(&ix, 1, decode_action).is_err());
    }

    #[test]
    fn decode_withdraw_rejects_bad_dest_tag() {
        // 0xFF is not a valid StandardSpk tag.
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        let mut body = Vec::new();
        body.push(ACTION_TAG_WITHDRAW);
        body.push(0u8); // user_idx
        body.extend_from_slice(&500u64.to_le_bytes());
        body.push(0xFF); // bad tag
        body.extend_from_slice(&[0u8; 32]);
        ix.extend_from_slice(&body);

        assert!(decode_ix(&ix, 1, decode_action).is_err());
    }
}
