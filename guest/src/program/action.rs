mod config;
mod deposit;
mod game;
mod user;
mod withdraw;

use config::{apply_init, apply_update};
use deposit::apply_deposit;
use game::{apply_create_game, apply_join_game, apply_timeout, apply_turn};
use user::{apply_transfer, apply_update_user_lock};
use vprogs_core_codec::{Error, Reader, Result as CodecResult};
use vprogs_core_types::ResourceId;
use vprogs_zk_abi::{
    Error as AbiError, Result as AbiResult, transaction_processor::Resource,
    withdrawal::StandardSpk,
};
use vprogs_zk_backend_risc0_runtime_processor::deposit_policy::DepositPolicy;
use withdraw::apply_withdraw;

// The battery's generic apply context with this app's auth context set by `runtime`;
// re-exported for one import site for the apply fns below.
pub use crate::runtime::ApplyContext;
use crate::{
    program::resources::{
        config::ConfigView, ext::ResourceExt, game::Cell, id::derive_user_resource,
    },
    runtime::{
        ix::read_resource_idx,
        lock::{LockEnum, decode_lock},
    },
};

/// Action discriminant on the ix wire: the byte that precedes every action body. Wire values
/// are fixed; new actions append.
#[repr(u8)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ActionTag {
    /// Update an existing config resource.
    Update = 0x01,
    /// Bootstrap (create) the singleton config resource. Gated by the env-provided genesis
    /// pubkey at apply time.
    Init = 0x02,
    /// Move balance between two user resources, creating the destination when its slot is new
    /// and a dest lock is supplied. Auth is checked against the source's current lock; the
    /// destination is not authed.
    Transfer = 0x03,
    /// Rotate the lock on a user resource. The current lock must authorize the rotation;
    /// `initial_lock_hash` is preserved.
    UpdateUserLock = 0x04,
    /// Credit a user from an L1 deposit output, creating the user when the slot is new (the
    /// sole path that creates a user resource). The funding output at `output_idx` of this tx
    /// must pay `DepositPolicy::deposit_spk(..)`; the credited amount is that output's `value`.
    Deposit = 0x05,
    /// Debit a user and emit an L2-to-L1 exit to `dest`. Authorized by the user's current
    /// lock; enforces `config.min_withdrawal_amount`.
    Withdraw = 0x06,
    /// Create an open staked game, locking `stake` from the creator's balance. Auth: the
    /// creator's user lock. The game slot must be new and carry
    /// `derive_game_resource(creator's initial_lock_hash, games_started)`.
    CreateGame = 0x07,
    /// Join an open game, locking the matching stake from the joiner's balance and starting
    /// play. Auth: the joiner's user lock.
    JoinGame = 0x08,
    /// Place a mark for a player of a playing game: applied when it is the mover's own ply,
    /// queued as a pre-commit otherwise. Auth: the mover's user lock.
    Turn = 0x09,
    /// Forfeit a round to the opponent when the to-move player's turn expired
    /// (`last_move_at + turn_ttl` elapsed). Permissionless: no lock is checked.
    Timeout = 0x0a,
}

impl TryFrom<u8> for ActionTag {
    type Error = ();

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0x01 => Ok(Self::Update),
            0x02 => Ok(Self::Init),
            0x03 => Ok(Self::Transfer),
            0x04 => Ok(Self::UpdateUserLock),
            0x05 => Ok(Self::Deposit),
            0x06 => Ok(Self::Withdraw),
            0x07 => Ok(Self::CreateGame),
            0x08 => Ok(Self::JoinGame),
            0x09 => Ok(Self::Turn),
            0x0a => Ok(Self::Timeout),
            _ => Err(()),
        }
    }
}

/// Read view over a single action entry.
pub struct ActionView<'a> {
    pub action_tag: ActionTag,
    pub body: ActionBody<'a>,
}

pub enum ActionBody<'a> {
    Update {
        config_idx: u8,
        new_min_withdrawal_amount: u64,
        /// New turn TTL, in milliseconds of chain time.
        new_turn_ttl: u64,
        /// Carried for wire-shape symmetry with `Init`. `apply_update` rejects
        /// any change here: covenant_id is immutable after `Init`.
        new_covenant_id: [u8; 32],
        new_lock: LockEnum<'a>,
    },
    Init {
        config_idx: u8,
        new_min_withdrawal_amount: u64,
        /// Turn TTL the config opens with, in milliseconds of chain time.
        new_turn_ttl: u64,
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
    CreateGame {
        /// Resource-list index of the creator's user resource; its lock authorizes the action.
        creator_idx: u8,
        /// Resource-list index of the game slot to create. Must be a new slot carrying exactly
        /// `derive_game_resource(creator's initial_lock_hash, games_started)`.
        game_idx: u8,
        /// Stake each seat locks into the pot (`2 x stake`).
        stake: u64,
        /// Match length in rounds; at least 1 (enforced at apply).
        rounds: u8,
        /// The creator's mark in even rounds; the joiner takes the other. X opens every round.
        mark: Cell,
    },
    JoinGame {
        /// Resource-list index of the open game being joined.
        game_idx: u8,
        /// Resource-list index of the joining user's resource; its lock authorizes the join
        /// and its id must differ from the creator's.
        joiner_idx: u8,
    },
    Turn {
        /// Resource-list index of the game being played.
        game_idx: u8,
        /// Resource-list index of the moving user's resource; its lock authorizes the turn
        /// and its id must name a player of the game.
        user_idx: u8,
        /// Board position 0..=8 the mover commits to.
        cell: u8,
    },
    Timeout {
        /// Resource-list index of the game whose to-move player's turn is claimed expired.
        game_idx: u8,
        /// Resource-list index of the config resource `turn_ttl` is read from. Must name the
        /// singleton config resource.
        config_idx: u8,
    },
}

/// The program's action decoder, handed to `runtime::ix::decode_ix`.
/// Decodes one action entry (`action_tag u8 || body`), bounds-checking every
/// resource index against `n_resources`.
pub fn decode_action<'a>(buf: &mut &'a [u8], n_resources: usize) -> CodecResult<ActionView<'a>> {
    let action_tag = ActionTag::try_from(buf.byte("action.action_tag")?)
        .map_err(|_| Error::Decode("action: unknown tag"))?;
    let body = match action_tag {
        ActionTag::Update => {
            let config_idx = read_resource_idx(buf, "action.update.config_idx", n_resources)?;
            let new_min_withdrawal_amount =
                buf.le_u64("action.update.new_min_withdrawal_amount")?;
            let new_turn_ttl = buf.le_u64("action.update.new_turn_ttl")?;
            let new_covenant_id = *buf.array::<32>("action.update.new_covenant_id")?;
            let new_lock = decode_lock(buf)?;
            ActionBody::Update {
                config_idx,
                new_min_withdrawal_amount,
                new_turn_ttl,
                new_covenant_id,
                new_lock,
            }
        }
        ActionTag::Init => {
            let config_idx = read_resource_idx(buf, "action.init.config_idx", n_resources)?;
            let new_min_withdrawal_amount = buf.le_u64("action.init.new_min_withdrawal_amount")?;
            let new_turn_ttl = buf.le_u64("action.init.new_turn_ttl")?;
            let new_covenant_id = *buf.array::<32>("action.init.new_covenant_id")?;
            let new_lock = decode_lock(buf)?;
            ActionBody::Init {
                config_idx,
                new_min_withdrawal_amount,
                new_turn_ttl,
                new_covenant_id,
                new_lock,
            }
        }
        ActionTag::Transfer => {
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
        ActionTag::UpdateUserLock => {
            let user_idx = read_resource_idx(buf, "action.update_user_lock.user_idx", n_resources)?;
            let new_lock = decode_lock(buf)?;
            ActionBody::UpdateUserLock { user_idx, new_lock }
        }
        ActionTag::Deposit => {
            let user_idx = read_resource_idx(buf, "action.deposit.user_idx", n_resources)?;
            let config_idx = read_resource_idx(buf, "action.deposit.config_idx", n_resources)?;
            let output_idx = buf.le_u32("action.deposit.output_idx")?;
            let initial_lock = decode_lock(buf)?;
            ActionBody::Deposit { user_idx, config_idx, output_idx, initial_lock }
        }
        ActionTag::Withdraw => {
            let user_idx = read_resource_idx(buf, "action.withdraw.user_idx", n_resources)?;
            let config_idx = read_resource_idx(buf, "action.withdraw.config_idx", n_resources)?;
            let amount = buf.le_u64("action.withdraw.amount")?;
            // StandardSpk::decode returns vprogs_zk_abi::Result; map into
            // the CodecResult this function returns.
            let dest = StandardSpk::decode(buf)
                .map_err(|_| Error::Decode("action.withdraw: bad dest spk"))?;
            ActionBody::Withdraw { user_idx, config_idx, amount, dest }
        }
        ActionTag::CreateGame => {
            let creator_idx =
                read_resource_idx(buf, "action.create_game.creator_idx", n_resources)?;
            let game_idx = read_resource_idx(buf, "action.create_game.game_idx", n_resources)?;
            let stake = buf.le_u64("action.create_game.stake")?;
            let rounds = buf.byte("action.create_game.rounds")?;
            // Mark byte mirrors the wire's Cell discriminants; Empty (0) is not choosable.
            let mark = match buf.byte("action.create_game.mark")? {
                1 => Cell::X,
                2 => Cell::O,
                _ => return Err(Error::Decode("action.create_game: mark must be X or O")),
            };
            ActionBody::CreateGame { creator_idx, game_idx, stake, rounds, mark }
        }
        ActionTag::JoinGame => {
            let game_idx = read_resource_idx(buf, "action.join_game.game_idx", n_resources)?;
            let joiner_idx = read_resource_idx(buf, "action.join_game.joiner_idx", n_resources)?;
            ActionBody::JoinGame { game_idx, joiner_idx }
        }
        ActionTag::Turn => {
            let game_idx = read_resource_idx(buf, "action.turn.game_idx", n_resources)?;
            let user_idx = read_resource_idx(buf, "action.turn.user_idx", n_resources)?;
            let cell = buf.byte("action.turn.cell")?;
            // A fixed-domain byte like the CreateGame mark: positions beyond the board are not
            // a semantic choice the apply layer should ever see.
            if cell > 8 {
                return Err(Error::Decode("action.turn: cell must be a board position 0..=8"));
            }
            ActionBody::Turn { game_idx, user_idx, cell }
        }
        ActionTag::Timeout => {
            let game_idx = read_resource_idx(buf, "action.timeout.game_idx", n_resources)?;
            let config_idx = read_resource_idx(buf, "action.timeout.config_idx", n_resources)?;
            ActionBody::Timeout { game_idx, config_idx }
        }
    };
    Ok(ActionView { action_tag, body })
}

/// Applies a single decoded action against the context. Generic over the deposit policy `P`; all
/// non-deposit arms ignore it.
pub fn apply_action<'a, P: DepositPolicy<Lock<'a> = LockEnum<'a>>>(
    action: &ActionView<'a>,
    cx: &mut ApplyContext<'a, '_>,
    policy: &P,
) -> AbiResult<()> {
    match &action.body {
        ActionBody::Update {
            config_idx: updater_idx,
            new_min_withdrawal_amount,
            new_turn_ttl,
            new_covenant_id,
            new_lock,
        } => apply_update(
            *updater_idx,
            *new_min_withdrawal_amount,
            *new_turn_ttl,
            new_covenant_id,
            new_lock,
            cx,
        ),
        ActionBody::Init {
            config_idx: updater_idx,
            new_min_withdrawal_amount,
            new_turn_ttl,
            new_covenant_id,
            new_lock,
        } => apply_init(
            *updater_idx,
            *new_min_withdrawal_amount,
            *new_turn_ttl,
            new_covenant_id,
            new_lock,
            cx,
        ),
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
        ActionBody::CreateGame { creator_idx, game_idx, stake, rounds, mark } => {
            apply_create_game(*creator_idx, *game_idx, *stake, *rounds, *mark, cx)
        }
        ActionBody::JoinGame { game_idx, joiner_idx } => {
            apply_join_game(*game_idx, *joiner_idx, cx)
        }
        ActionBody::Turn { game_idx, user_idx, cell } => {
            apply_turn(*game_idx, *user_idx, *cell, cx)
        }
        ActionBody::Timeout { game_idx, config_idx } => apply_timeout(*game_idx, *config_idx, cx),
    }
}

/// Reads a config field via the resource at `config_idx`, for actions that address the
/// config by index instead of scanning the resource list.
///
/// No id derivation runs here: `Init` is the config's only birth path and it enforces the
/// derived id, so a live config-kind slot is the singleton by construction. `view_config`'s
/// `None` (wrong kind, malformed, or emptied slot) rejects everything else. The decoder has
/// already bounds-checked the index against `n_resources`.
pub(super) fn view_config_at<R>(
    resources: &[Resource<'_>],
    config_idx: u8,
    f: impl FnOnce(&ConfigView) -> R,
) -> AbiResult<R> {
    resources[config_idx as usize]
        .view_config(f)
        .ok_or_else(|| AbiError::Decode("config resource not live".into()))
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
    /// `updater_idx u8 || new_min u64 || new_turn_ttl u64 || covenant_id[32] || schnorr_lock(pk)`.
    fn update_action(updater_idx: u8, new_min: u64, new_turn_ttl: u64, pk: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ActionTag::Update as u8);
        body.push(updater_idx);
        body.extend_from_slice(&new_min.to_le_bytes());
        body.extend_from_slice(&new_turn_ttl.to_le_bytes());
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
        ix.extend_from_slice(&update_action(0, 12345, 60_000, [0xAAu8; 32]));

        let decoded = decode_ix(&ix, 1, decode_action).unwrap();
        assert!(decoded.signers.is_empty());
        assert_eq!(decoded.actions.len(), 1);
        match &decoded.actions[0].body {
            ActionBody::Update {
                config_idx: updater_idx,
                new_min_withdrawal_amount,
                new_turn_ttl,
                new_covenant_id,
                new_lock,
            } => {
                assert_eq!(*updater_idx, 0);
                assert_eq!(*new_min_withdrawal_amount, 12345);
                assert_eq!(*new_turn_ttl, 60_000);
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
        ix.extend_from_slice(&update_action(2, 12345, 60_000, [0xAAu8; 32]));

        assert!(decode_ix(&ix, 2, decode_action).is_err());
    }

    #[test]
    fn decode_action_accepts_updater_idx_at_upper_bound() {
        // updater_idx = 1 with n_resources = 2 is valid.
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&update_action(1, 12345, 60_000, [0xAAu8; 32]));

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
            ix.extend_from_slice(&update_action(idx, 999, 60_000, [0xBBu8; 32]));

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
        ix.extend_from_slice(&update_action(0, 100, 60_000, [0x11u8; 32]));
        ix.extend_from_slice(&update_action(2, 200, 60_000, [0x22u8; 32]));

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
        ix.extend_from_slice(&update_action(1, 100, 60_000, [0x33u8; 32])); // valid
        ix.extend_from_slice(&update_action(5, 200, 60_000, [0x44u8; 32])); // out of range

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
        ix.extend_from_slice(&update_action(2, 1, 60_000, [0x55u8; 32]));
        ix.extend_from_slice(&update_action(1, 2, 60_000, [0x66u8; 32]));
        ix.extend_from_slice(&update_action(0, 3, 60_000, [0x77u8; 32]));

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
        body.push(ActionTag::Transfer as u8);
        body.push(source);
        body.push(dest);
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(0); // has_dest_lock = 0 (credit existing)
        body
    }

    fn transfer_create_action(source: u8, dest: u8, amount: u64, pk: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ActionTag::Transfer as u8);
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
        body.push(ActionTag::UpdateUserLock as u8);
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
        body.push(ActionTag::Transfer as u8);
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
        body.push(ActionTag::Deposit as u8);
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
        body.push(ActionTag::Withdraw as u8);
        body.push(user_idx);
        body.push(0); // config_idx
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(0x00); // StandardSpk::PubKey tag
        body.extend_from_slice(&pk);
        body
    }

    fn withdraw_action_pubkey_ecdsa(user_idx: u8, amount: u64, pk: [u8; 33]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ActionTag::Withdraw as u8);
        body.push(user_idx);
        body.push(0); // config_idx
        body.extend_from_slice(&amount.to_le_bytes());
        body.push(0x01); // StandardSpk::PubKeyEcdsa tag
        body.extend_from_slice(&pk);
        body
    }

    fn withdraw_action_script_hash(user_idx: u8, amount: u64, hash: [u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ActionTag::Withdraw as u8);
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
        body.push(ActionTag::Withdraw as u8);
        body.push(0u8); // user_idx
        body.extend_from_slice(&500u64.to_le_bytes());
        body.push(0xFF); // bad tag
        body.extend_from_slice(&[0u8; 32]);
        ix.extend_from_slice(&body);

        assert!(decode_ix(&ix, 1, decode_action).is_err());
    }

    // CreateGame / JoinGame decoder arms

    /// Builds a CreateGame action: `tag | creator_idx | game_idx | stake(8 LE) | rounds | mark`.
    fn create_game_action(
        creator_idx: u8,
        game_idx: u8,
        stake: u64,
        rounds: u8,
        mark: u8,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(ActionTag::CreateGame as u8);
        body.push(creator_idx);
        body.push(game_idx);
        body.extend_from_slice(&stake.to_le_bytes());
        body.push(rounds);
        body.push(mark);
        body
    }

    fn join_game_action(game_idx: u8, joiner_idx: u8) -> Vec<u8> {
        vec![ActionTag::JoinGame as u8, game_idx, joiner_idx]
    }

    #[test]
    fn decode_create_game_action() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&create_game_action(0, 1, 5_000, 3, 2));

        let decoded = decode_ix(&ix, 2, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::CreateGame { creator_idx, game_idx, stake, rounds, mark } => {
                assert_eq!(*creator_idx, 0);
                assert_eq!(*game_idx, 1);
                assert_eq!(*stake, 5_000);
                assert_eq!(*rounds, 3);
                assert_eq!(*mark, Cell::O);
            }
            _ => panic!("expected CreateGame"),
        }
    }

    #[test]
    fn decode_create_game_accepts_both_marks() {
        for (wire, want) in [(1u8, Cell::X), (2u8, Cell::O)] {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&create_game_action(0, 1, 5_000, 3, wire));

            let decoded = decode_ix(&ix, 2, decode_action).unwrap();
            match &decoded.actions[0].body {
                ActionBody::CreateGame { mark, .. } => assert_eq!(*mark, want),
                _ => panic!("expected CreateGame"),
            }
        }
    }

    #[test]
    fn decode_create_game_rejects_bad_mark() {
        // 0 is Empty (not choosable), 3 is not a Cell discriminant.
        for bad in [0u8, 3u8] {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&create_game_action(0, 1, 5_000, 3, bad));

            assert!(decode_ix(&ix, 2, decode_action).is_err());
        }
    }

    #[test]
    fn decode_create_game_rejects_out_of_range_idx() {
        // creator_idx = 2 then game_idx = 2, each with only 2 resources declared.
        for action in [create_game_action(2, 1, 5_000, 3, 1), create_game_action(0, 2, 5_000, 3, 1)]
        {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&action);

            assert!(decode_ix(&ix, 2, decode_action).is_err());
        }
    }

    #[test]
    fn decode_join_game_action() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&join_game_action(0, 1));

        let decoded = decode_ix(&ix, 2, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::JoinGame { game_idx, joiner_idx } => {
                assert_eq!(*game_idx, 0);
                assert_eq!(*joiner_idx, 1);
            }
            _ => panic!("expected JoinGame"),
        }
    }

    #[test]
    fn decode_join_game_rejects_out_of_range_idx() {
        for action in [join_game_action(2, 1), join_game_action(0, 2)] {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&action);

            assert!(decode_ix(&ix, 2, decode_action).is_err());
        }
    }

    // Turn decoder arm

    /// Builds a Turn action: `tag | game_idx | user_idx | cell`.
    fn turn_action(game_idx: u8, user_idx: u8, cell: u8) -> Vec<u8> {
        vec![ActionTag::Turn as u8, game_idx, user_idx, cell]
    }

    #[test]
    fn decode_turn_action() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&turn_action(0, 2, 8));

        let decoded = decode_ix(&ix, 3, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Turn { game_idx, user_idx, cell } => {
                assert_eq!(*game_idx, 0);
                assert_eq!(*user_idx, 2);
                assert_eq!(*cell, 8);
            }
            _ => panic!("expected Turn"),
        }
    }

    #[test]
    fn decode_turn_rejects_cell_off_board() {
        for bad in [9u8, 255] {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&turn_action(0, 1, bad));

            assert!(decode_ix(&ix, 2, decode_action).is_err());
        }
    }

    #[test]
    fn decode_turn_rejects_out_of_range_idx() {
        for action in [turn_action(2, 1, 0), turn_action(0, 2, 0)] {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&action);

            assert!(decode_ix(&ix, 2, decode_action).is_err());
        }
    }

    // Timeout decoder arm

    /// Builds a Timeout action: `tag | game_idx | config_idx`.
    fn timeout_action(game_idx: u8, config_idx: u8) -> Vec<u8> {
        vec![ActionTag::Timeout as u8, game_idx, config_idx]
    }

    #[test]
    fn decode_timeout_action() {
        let mut ix = 0u32.to_le_bytes().to_vec();
        ix.extend_from_slice(&1u32.to_le_bytes());
        ix.extend_from_slice(&timeout_action(1, 3));

        let decoded = decode_ix(&ix, 4, decode_action).unwrap();
        match &decoded.actions[0].body {
            ActionBody::Timeout { game_idx, config_idx } => {
                assert_eq!(*game_idx, 1);
                assert_eq!(*config_idx, 3);
            }
            _ => panic!("expected Timeout"),
        }
    }

    #[test]
    fn decode_timeout_rejects_out_of_range_idx() {
        for action in [timeout_action(2, 1), timeout_action(0, 2)] {
            let mut ix = 0u32.to_le_bytes().to_vec();
            ix.extend_from_slice(&1u32.to_le_bytes());
            ix.extend_from_slice(&action);

            assert!(decode_ix(&ix, 2, decode_action).is_err());
        }
    }

    // ActionTag <-> wire byte

    /// Every variant maps to exactly one wire byte and back; the bounds (0, 0x0B) reject.
    /// A new variant added without a `TryFrom` arm fails here.
    #[test]
    fn action_tag_round_trips_every_variant() {
        for tag in [
            ActionTag::Update,
            ActionTag::Init,
            ActionTag::Transfer,
            ActionTag::UpdateUserLock,
            ActionTag::Deposit,
            ActionTag::Withdraw,
            ActionTag::CreateGame,
            ActionTag::JoinGame,
            ActionTag::Turn,
            ActionTag::Timeout,
        ] {
            assert_eq!(ActionTag::try_from(tag as u8), Ok(tag));
        }
        assert!(ActionTag::try_from(0x00).is_err());
        assert!(ActionTag::try_from(0x0B).is_err());
        assert!(ActionTag::try_from(0xFF).is_err());
    }
}
