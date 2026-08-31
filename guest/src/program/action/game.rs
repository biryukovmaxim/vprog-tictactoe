//! Game actions: `CreateGame` (open a staked match), `JoinGame` (fill the second seat and
//! start play), `Turn` (place a mark, run the pre-commit cascade, settle the match), and
//! `Timeout` (forfeit a round whose to-move player let the clock expire).
//!
//! `Timeout` is the only game action that reads config: it compares the mergeset DAA score
//! against `last_move_at + turn_ttl`. Stake and rounds are explicit `CreateGame` parameters,
//! so nothing else needs a config read.
use vprogs_core_types::ResourceId;
use vprogs_zk_abi::{Error as AbiError, Result as AbiResult, transaction_processor::Resource};
use vprogs_zk_backend_risc0_runtime_processor::lifecycle::Lifecycle;

use super::{ApplyContext, view_config_at};
use crate::program::{
    resources::{
        ext::ResourceExt,
        game::{Cell, GameBody, State},
        id::derive_game_resource,
    },
    rules,
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
            v.games_started_mut()
                .set(games_started.checked_add(1).ok_or("create_game: games_started overflow")?);
            Ok::<(), &'static str>(())
        })
        .ok_or_else(|| {
            AbiError::Decode("create_game: creator not a live writable user resource".into())
        })?;
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
/// `last_move_at` with the mergeset DAA score so the turn-TTL window starts at match start.
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
        .ok_or_else(|| {
            AbiError::Decode("join_game: joiner not a live writable user resource".into())
        })?;
    debit.map_err(|m| AbiError::Decode(m.into()))?;

    let now = cx.context.daa_score.get();
    cx.resources[game_idx as usize]
        .modify_game(|g| {
            g.set_joiner(&joiner_id);
            g.set_state(State::Playing);
            g.set_last_move_at(now);
        })
        .ok_or_else(|| AbiError::Decode("join_game: not a live writable game resource".into()))
}

/// Places a mark for a player of a playing game, authored by the mover's user lock.
///
/// A turn for the mover's own ply lands on the board at once; a turn for a future ply queues as
/// the mover's pre-commit. After the explicit turn, the cascade applies the to-move seat's
/// queued head whenever it names an open cell (stale heads on taken cells are dropped), so one
/// tx carrying live moves plus pre-commits can play out many plies.
///
/// When a turn finishes the match (final round or early clinch), the pot settles: winner takes
/// `2 x stake`, a draw returns each seat's stake, and both players' user resources (located by
/// id in this tx's resource list) take their `games_finished`/`games_won` bumps. Both players
/// must be attached and declared writable in the finalizing tx, or the action rejects.
pub(super) fn apply_turn(
    game_idx: u8,
    user_idx: u8,
    cell: u8,
    cx: &mut ApplyContext<'_, '_>,
) -> AbiResult<()> {
    let auth_ok = cx.resources[user_idx as usize]
        .view_user(|v| v.lock().unlock(user_idx, cx.auth_ctx))
        .ok_or_else(|| AbiError::Decode("turn: mover not a live user resource".into()))?;
    if !auth_ok {
        return Err(AbiError::Decode("turn: mover lock not satisfied".into()));
    }
    let mover_id = *cx.resources[user_idx as usize].id();
    let now = cx.context.daa_score.get();

    let finished = cx.resources[game_idx as usize]
        .modify_game(|g| play_turn(g, &mover_id, cell, now))
        .ok_or_else(|| AbiError::Decode("turn: not a live writable game resource".into()))?
        .map_err(|m| AbiError::Decode(m.into()))?;
    if finished {
        settle_match(cx, game_idx)?;
    }
    Ok(())
}

/// Forfeits the current round of a playing game to the seat not to move, once its turn expired:
/// the mergeset DAA score reached `last_move_at + turn_ttl` (the config's, read via
/// `config_idx`). Permissionless: the clock check is the whole authority, so anyone may sweep
/// an expired turn.
///
/// The forfeited round closes like a won one (board reset, early-clinch check), and the
/// winner's queued pre-commits drain into the fresh round, mirroring `Turn`'s cascade. When the
/// forfeit finishes the match, settlement runs exactly as after a `Turn`.
pub(super) fn apply_timeout(
    game_idx: u8,
    config_idx: u8,
    cx: &mut ApplyContext<'_, '_>,
) -> AbiResult<()> {
    let turn_ttl = view_config_at(cx.resources, config_idx, |c| c.turn_ttl())?;
    let now = cx.context.daa_score.get();

    let finished = cx.resources[game_idx as usize]
        .modify_game(|g| forfeit_round(g, turn_ttl, now))
        .ok_or_else(|| AbiError::Decode("timeout: not a live writable game resource".into()))?
        .map_err(|m| AbiError::Decode(m.into()))?;
    if finished {
        settle_match(cx, game_idx)?;
    }
    Ok(())
}

/// Pays out a just-finished match's pot and bumps both players' stats. Both players' user
/// resources must be attached to this tx and declared writable, or the settlement rejects the
/// finalizing action and the submitter retries with them included.
fn settle_match(cx: &mut ApplyContext<'_, '_>, game_idx: u8) -> AbiResult<()> {
    let (state, players, stake) = {
        let (state, creator, joiner, stake) = cx.resources[game_idx as usize]
            .view_game(|g| (g.state(), *g.creator(), g.joiner().copied(), g.stake()))
            .ok_or_else(|| AbiError::Decode("settle: not a live game resource".into()))?;
        match joiner {
            Some(joiner) => (state, [creator, joiner], stake),
            None => return Err(AbiError::Decode("settle: finished game without a joiner".into())),
        }
    };

    let pot =
        stake.checked_mul(2).ok_or_else(|| AbiError::Decode("settle: pot overflows".into()))?;
    let (credits, winner): ([u64; 2], Option<usize>) = match state {
        State::First => ([pot, 0], Some(0)),
        State::Second => ([0, pot], Some(1)),
        State::Draw => ([stake, stake], None),
        State::Open | State::Playing => {
            return Err(AbiError::Decode("settle: settlement on an unfinished game".into()));
        }
    };

    for (seat, (player, credit)) in players.into_iter().zip(credits).enumerate() {
        let idx = user_slot_of(cx.resources, &player).ok_or_else(|| {
            AbiError::Decode("settle: player user resource missing from tx".into())
        })?;
        let won = winner == Some(seat);
        let update = cx.resources[idx]
            .modify_user(|v| {
                if credit > 0 {
                    let bal = v.balance_mut();
                    bal.set(bal.get().checked_add(credit).ok_or("settle: balance overflow")?);
                }
                let finished = v.games_finished_mut();
                finished.set(finished.get().checked_add(1).ok_or("settle: stat overflow")?);
                if won {
                    let won = v.games_won_mut();
                    won.set(won.get().checked_add(1).ok_or("settle: stat overflow")?);
                }
                Ok::<(), &'static str>(())
            })
            .ok_or_else(|| {
                AbiError::Decode("settle: player not a live writable user resource".into())
            })?;
        update.map_err(|m| AbiError::Decode(m.into()))?;
    }
    Ok(())
}

/// Plays the explicit turn and its cascade, all under the caller's `modify_game` borrow, and
/// returns whether the match just finished. Every check that can fail runs before the first
/// board or queue write.
fn play_turn(
    g: &mut GameBody,
    mover_id: &ResourceId,
    cell: u8,
    now: u64,
) -> Result<bool, &'static str> {
    if g.state() != State::Playing {
        return Err("turn: game is not playing");
    }
    let seat = seat_of(g, mover_id)?;
    commit_explicit(g, seat, cell, now)?;
    drain_pending(g, now);
    Ok(g.is_finished())
}

/// Lands the explicit turn: on the board when it is the mover's own ply, else queued as the
/// mover's pre-commit.
fn commit_explicit(g: &mut GameBody, seat: usize, cell: u8, now: u64) -> Result<(), &'static str> {
    if g.board()[cell as usize] != Cell::Empty {
        return Err("turn: cell is occupied");
    }
    if seat == to_move(g) {
        apply_move(g, seat, cell, now);
    } else if !g.pending_mut(seat).push(cell) {
        return Err("turn: pending queue is full");
    }
    Ok(())
}

/// Applies queued pre-commits while the game plays: each applied move hands the turn to the
/// other seat, whose own head applies next, and so on across round boundaries (queues are
/// ply-agnostic). Heads naming taken cells are stale and dropped. Every iteration pops exactly
/// one entry, so the loop terminates.
fn drain_pending(g: &mut GameBody, now: u64) {
    while g.state() == State::Playing {
        let seat = to_move(g);
        let Some(cell) = g.pending_mut(seat).peek() else { break };
        g.pending_mut(seat).pop();
        if g.board()[cell as usize] == Cell::Empty {
            apply_move(g, seat, cell, now);
        }
    }
}

/// Awards the round to the seat not to move when `now` passed `last_move_at + turn_ttl`, and
/// returns whether the match just finished. The claim itself stamps `last_move_at`, so the
/// fresh round's clock starts at the claim: a same-tx repeat sees `now < deadline` and rejects.
fn forfeit_round(g: &mut GameBody, turn_ttl: u64, now: u64) -> Result<bool, &'static str> {
    if g.state() != State::Playing {
        return Err("timeout: game is not playing");
    }
    let deadline = g
        .last_move_at()
        .checked_add(turn_ttl)
        .ok_or("timeout: last_move_at + turn_ttl overflows")?;
    if now < deadline {
        return Err("timeout: turn has not expired");
    }

    // XOR flips a 0/1 seat index with no underflow edge.
    let winner = to_move(g) ^ 1;
    g.round_wins_mut()[winner] += 1;
    g.set_last_move_at(now);
    close_round(g);
    drain_pending(g, now);
    Ok(g.is_finished())
}

/// Places `seat`'s mark at `cell`, stamps the clock, and closes the round when the move ends it
/// (a completed line or a full board).
fn apply_move(g: &mut GameBody, seat: usize, cell: u8, now: u64) {
    let (creator_mark, round) = mark_and_round(g);
    let mark = rules::mark_for_seat(creator_mark, round, seat);
    g.board_mut()[cell as usize] = mark;
    g.set_last_move_at(now);

    // The mover's own line is the only one this move can complete; a full board with no line is
    // the round draw.
    let won = rules::winner(g.board()) == Some(mark);
    let full = !won && rules::board_full(g.board());
    if won {
        g.round_wins_mut()[seat] += 1;
    } else if full {
        *g.draws_mut() += 1;
    }
    if won || full {
        close_round(g);
    }
}

/// Resets the board for the next round, then ends the match when the counters decide it.
fn close_round(g: &mut GameBody) {
    g.board_mut().fill(Cell::Empty);
    end_match_if_decided(g);
}

/// Ends the match (terminal state, queues zeroed) when the counters decide it: an early clinch
/// or the final round played.
fn end_match_if_decided(g: &mut GameBody) {
    let outcome = rules::match_outcome(g.rounds_total(), &g.round_wins(), g.draws());
    if let Some(state) = outcome {
        g.set_state(state);
        g.clear_pending();
    }
}

/// The mover's seat in the game, or an error when their resource id names neither player.
fn seat_of(v: &GameBody, mover_id: &ResourceId) -> Result<usize, &'static str> {
    if v.creator() == mover_id {
        Ok(0)
    } else if v.joiner() == Some(mover_id) {
        Ok(1)
    } else {
        Err("turn: mover is not a player of this game")
    }
}

/// The seat whose ply it is on the current board.
fn to_move(v: &GameBody) -> usize {
    let (creator_mark, round) = mark_and_round(v);
    rules::seat_to_move(creator_mark, round, v.board())
}

/// Creator mark and current round index (rounds played so far, derived from the counters).
fn mark_and_round(v: &GameBody) -> (Cell, u8) {
    let wins = v.round_wins();
    (v.creator_mark(), wins[0] + wins[1] + v.draws())
}

/// Position of the user resource carrying `id` in the tx resource list, if attached.
fn user_slot_of(resources: &[Resource<'_>], id: &ResourceId) -> Option<usize> {
    resources.iter().position(|r| r.id() == id)
}
