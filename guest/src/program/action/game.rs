//! Game actions: `CreateGame` (open a staked match) and `JoinGame` (fill the second seat and
//! start play).
//!
//! Game actions read no config: stake and rounds are explicit `CreateGame` parameters, and the
//! only time-dependent behavior (the turn timeout) belongs to a later action that compares the
//! mergeset clock against `last_move_at` + `turn_ttl`.

use vprogs_zk_abi::{Error as AbiError, Result as AbiResult};
use vprogs_zk_backend_risc0_runtime_processor::lifecycle::Lifecycle;

use super::ApplyContext;
use crate::program::resources::{
    ext::ResourceExt,
    game::{Cell, State},
    id::derive_game_resource,
};

/// Creates an open staked game at `game_idx`, authored by the creator's user lock.
///
/// The game slot must be new and carry exactly `derive_game_resource(creator's
/// initial_lock_hash, games_started)`: the id check binds the counter-derived address the same
/// way `validate_user_create` binds a user's, so the deterministic id scheme can be trusted.
/// Creation locks `stake` from the creator's balance and bumps their `games_started`, making
/// the next game derive a fresh id.
///
/// All reads and checks complete before any state mutation, so a failed check leaves nothing
/// behind.
pub(super) fn apply_create_game(
    creator_idx: u8,
    game_idx: u8,
    stake: u64,
    rounds: u8,
    mark: Cell,
    cx: &mut ApplyContext<'_, '_>,
) -> AbiResult<()> {
    if rounds == 0 {
        return Err(AbiError::Decode("create_game: rounds must be at least 1".into()));
    }
    // A non-New slot means this deterministic id already exists (or was torn down earlier in
    // the tx); either way the game cannot be born here.
    if !matches!(cx.lifecycle(game_idx as usize), Lifecycle::New) {
        return Err(AbiError::Decode("create_game: game slot is not new".into()));
    }

    // Kind and liveness of the creator fold into the combinator: `view_user` returns `None`
    // for wrong-kind or empty slots.
    let (auth_ok, ilh, games_started) = cx.resources[creator_idx as usize]
        .view_user(|v| {
            (v.lock().unlock(creator_idx, cx.auth_ctx), *v.initial_lock_hash(), v.games_started())
        })
        .ok_or_else(|| AbiError::Decode("create_game: creator not a live user resource".into()))?;
    if !auth_ok {
        return Err(AbiError::Decode("create_game: creator lock not satisfied".into()));
    }

    let expected_id = derive_game_resource(&ilh, games_started);
    if cx.resources[game_idx as usize].id() != &expected_id {
        return Err(AbiError::Decode(
            "create_game: slot id != derive_game_resource(initial_lock_hash, games_started)".into(),
        ));
    }

    // Debit the stake and advance the derivation counter in one borrow; `checked_sub` is the
    // balance check, failing before either field is written.
    let debit = cx.resources[creator_idx as usize]
        .modify_user(|v| {
            let bal = v.balance_mut();
            let new = bal.get().checked_sub(stake).ok_or("create_game: insufficient balance")?;
            bal.set(new);
            v.games_started_mut().set(games_started + 1);
            Ok::<(), &'static str>(())
        })
        .ok_or_else(|| AbiError::Decode("create_game: creator not a live user resource".into()))?;
    debit.map_err(|m| AbiError::Decode(m.into()))?;

    // Advance `New -> Live` (rejecting double-create) before writing the fresh payload.
    cx.mark_created(game_idx as usize).map_err(|m| AbiError::Decode(m.into()))?;
    let creator_id = *cx.resources[creator_idx as usize].id();
    cx.resources[game_idx as usize]
        .init_game(&creator_id, mark, stake, rounds)
        .map_err(|m| AbiError::Decode(m.into()))
}

/// Joins the open game at `game_idx`, authored by the joiner's user lock. Locks the game's
/// stake from the joiner's balance, fills seat 1, moves the match to `Playing`, and stamps
/// `last_move_at` with the mergeset clock so the turn-TTL window starts at match start.
///
/// The state check subsumes every other shape of bad join: a finished or in-progress game is
/// not `Open`, and a non-game or non-live slot fails in `view_game`.
pub(super) fn apply_join_game(
    game_idx: u8,
    joiner_idx: u8,
    cx: &mut ApplyContext<'_, '_>,
) -> AbiResult<()> {
    let auth_ok = cx.resources[joiner_idx as usize]
        .view_user(|v| v.lock().unlock(joiner_idx, cx.auth_ctx))
        .ok_or_else(|| AbiError::Decode("join_game: joiner not a live user resource".into()))?;
    if !auth_ok {
        return Err(AbiError::Decode("join_game: joiner lock not satisfied".into()));
    }

    let (state, creator_id, stake) = cx.resources[game_idx as usize]
        .view_game(|g| (g.state(), *g.creator(), g.stake()))
        .ok_or_else(|| AbiError::Decode("join_game: not a live game resource".into()))?;
    if state != State::Open {
        return Err(AbiError::Decode("join_game: game is not open".into()));
    }

    let joiner_id = *cx.resources[joiner_idx as usize].id();
    if joiner_id == creator_id {
        return Err(AbiError::Decode("join_game: joiner is the creator".into()));
    }

    // Debit the matching stake; `checked_sub` fails the action before the game is touched.
    let debit = cx.resources[joiner_idx as usize]
        .modify_user(|v| {
            let bal = v.balance_mut();
            let new = bal.get().checked_sub(stake).ok_or("join_game: insufficient balance")?;
            bal.set(new);
            Ok::<(), &'static str>(())
        })
        .ok_or_else(|| AbiError::Decode("join_game: joiner not a live user resource".into()))?;
    debit.map_err(|m| AbiError::Decode(m.into()))?;

    let now = cx.context.timestamp.get();
    cx.resources[game_idx as usize]
        .modify_game(|g| {
            g.set_joiner(&joiner_id);
            g.set_state(State::Playing);
            g.set_last_move_at(now);
        })
        .ok_or_else(|| AbiError::Decode("join_game: not a live game resource".into()))
}
