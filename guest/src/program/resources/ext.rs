//! Closure-based combinators on `Resource<'a>` that layer typed views onto
//! the byte-backed storage.
//!
//! Every mutating combinator requires the tx to have declared the resource `AccessType::Write`:
//! nothing on the guest path enforces the declared access mode, so this module is the single
//! choke point through which all action writes pass. Read-only (`view_*`) combinators accept
//! both modes.
//!
//! Liveness and kind fold into the bodies' validating `from_bytes`: it rejects empty data (a
//! `New` slot has nothing yet, a `Deleted` one was emptied on teardown) and any slot whose
//! payload is not of the view's kind, so `None` means "not a live resource of that kind".
//! The lifecycle machinery itself lives in `ApplyContext`.

use vprogs_core_types::{AccessType, ResourceId};
use vprogs_zk_abi::transaction_processor::Resource;

use crate::{
    program::resources::{
        config::{CONFIG_HEADER_LEN, ConfigBody, config_total_len, write_config},
        game::{Cell, GAME_WIRE_LEN, GameBody, write_game},
        user::{GameStats, USER_HEADER_LEN, UserBody, user_total_len, write_user},
    },
    runtime::lock::LockEnum,
};

/// Whether the tx declared this resource writable; the gate every mutating combinator checks.
fn is_writable(r: &Resource<'_>) -> bool {
    r.access_type() == AccessType::Write
}

/// Extension trait over `Resource<'a>`; see module docs.
pub trait ResourceExt {
    /// Runs `f` on the config payload; `None` when the slot holds no live config.
    fn view_config<R>(&self, f: impl FnOnce(&ConfigBody) -> R) -> Option<R>;
    /// Runs `f` on the user payload; `None` when the slot holds no live user.
    fn view_user<R>(&self, f: impl FnOnce(&UserBody) -> R) -> Option<R>;
    /// Runs `f` on the game payload; `None` when the slot holds no live game.
    fn view_game<R>(&self, f: impl FnOnce(&GameBody) -> R) -> Option<R>;

    /// Mutably runs `f` on the config payload; `None` when not live or not declared writable.
    fn modify_config<R>(&mut self, f: impl FnOnce(&mut ConfigBody) -> R) -> Option<R>;
    /// Mutably runs `f` on the user payload; `None` when not live or not declared writable.
    fn modify_user<R>(&mut self, f: impl FnOnce(&mut UserBody) -> R) -> Option<R>;
    /// Mutably runs `f` on the game payload; `None` when not live or not declared writable.
    fn modify_game<R>(&mut self, f: impl FnOnce(&mut GameBody) -> R) -> Option<R>;

    /// Creates the config payload in an empty writable slot.
    fn init_config(
        &mut self,
        min_withdrawal_amount: u64,
        turn_ttl: u64,
        covenant_id: &[u8; 32],
        lock: &LockEnum<'_>,
    ) -> Result<(), &'static str>;
    /// Creates a user payload in an empty writable slot; game counters start at zero.
    fn init_user(
        &mut self,
        balance: u64,
        initial_lock_hash: &[u8; 32],
        lock: &LockEnum<'_>,
    ) -> Result<(), &'static str>;
    /// Creates an open game payload in an empty writable slot.
    fn init_game(
        &mut self,
        creator: &ResourceId,
        creator_mark: Cell,
        stake: u64,
        rounds_total: u8,
    ) -> Result<(), &'static str>;

    /// Rewrites the config's lock, preserving the fixed header.
    fn set_config_lock(&mut self, new_lock: &LockEnum<'_>) -> Result<(), &'static str>;
    /// Rewrites the user's lock, preserving balance, counters, and `initial_lock_hash`.
    fn set_user_lock(&mut self, new_lock: &LockEnum<'_>) -> Result<(), &'static str>;
}

impl ResourceExt for Resource<'_> {
    fn view_config<R>(&self, f: impl FnOnce(&ConfigBody) -> R) -> Option<R> {
        Some(f(ConfigBody::from_bytes(self.data()).ok()?))
    }

    fn view_user<R>(&self, f: impl FnOnce(&UserBody) -> R) -> Option<R> {
        Some(f(UserBody::from_bytes(self.data()).ok()?))
    }

    fn view_game<R>(&self, f: impl FnOnce(&GameBody) -> R) -> Option<R> {
        Some(f(GameBody::from_bytes(self.data()).ok()?))
    }

    fn modify_config<R>(&mut self, f: impl FnOnce(&mut ConfigBody) -> R) -> Option<R> {
        if !is_writable(self) {
            return None;
        }
        let mv = ConfigBody::from_bytes_mut(self.data_mut()).ok()?;
        Some(f(mv))
    }

    fn modify_user<R>(&mut self, f: impl FnOnce(&mut UserBody) -> R) -> Option<R> {
        if !is_writable(self) {
            return None;
        }
        let mv = UserBody::from_bytes_mut(self.data_mut()).ok()?;
        Some(f(mv))
    }

    fn modify_game<R>(&mut self, f: impl FnOnce(&mut GameBody) -> R) -> Option<R> {
        if !is_writable(self) {
            return None;
        }
        let mv = GameBody::from_bytes_mut(self.data_mut()).ok()?;
        Some(f(mv))
    }

    fn init_config(
        &mut self,
        min_withdrawal_amount: u64,
        turn_ttl: u64,
        covenant_id: &[u8; 32],
        lock: &LockEnum<'_>,
    ) -> Result<(), &'static str> {
        if !is_writable(self) {
            return Err("init_config: resource not declared writable");
        }
        if !self.data().is_empty() {
            return Err("init_config: slot is not empty");
        }
        let total = config_total_len(lock);
        self.resize(total);
        write_config(self.data_mut(), min_withdrawal_amount, turn_ttl, covenant_id, lock)
    }

    fn init_user(
        &mut self,
        balance: u64,
        initial_lock_hash: &[u8; 32],
        lock: &LockEnum<'_>,
    ) -> Result<(), &'static str> {
        if !is_writable(self) {
            return Err("init_user: resource not declared writable");
        }
        if !self.data().is_empty() {
            return Err("init_user: slot is not empty");
        }
        let total = user_total_len(lock);
        self.resize(total);
        // Fresh accounts have no game history; the game actions are the only writers.
        write_user(self.data_mut(), balance, GameStats::default(), initial_lock_hash, lock)
    }

    fn init_game(
        &mut self,
        creator: &ResourceId,
        creator_mark: Cell,
        stake: u64,
        rounds_total: u8,
    ) -> Result<(), &'static str> {
        if !is_writable(self) {
            return Err("init_game: resource not declared writable");
        }
        if !self.data().is_empty() {
            return Err("init_game: slot is not empty");
        }
        self.resize(GAME_WIRE_LEN);
        write_game(self.data_mut(), creator, creator_mark, stake, rounds_total)
    }

    fn set_config_lock(&mut self, new_lock: &LockEnum<'_>) -> Result<(), &'static str> {
        if !is_writable(self) {
            return Err("set_config_lock: resource not declared writable");
        }
        // Snapshot fixed-header fields before resize (which would invalidate
        // the existing data slice).
        let (min_withdrawal_amount, turn_ttl, covenant_id) = {
            let view = ConfigBody::from_bytes(self.data())
                .map_err(|_| "set_config_lock: not a config resource")?;
            (view.min_withdrawal_amount(), view.turn_ttl(), *view.covenant_id())
        };
        let new_total = CONFIG_HEADER_LEN + new_lock.wire_body_len();
        self.resize(new_total);
        write_config(self.data_mut(), min_withdrawal_amount, turn_ttl, &covenant_id, new_lock)
    }

    fn set_user_lock(&mut self, new_lock: &LockEnum<'_>) -> Result<(), &'static str> {
        if !is_writable(self) {
            return Err("set_user_lock: resource not declared writable");
        }
        // Snapshot the fixed fields (balance + game counters) before resize; the rotation
        // must not reset game history.
        let (balance, stats, ilh) = {
            let view = UserBody::from_bytes(self.data())
                .map_err(|_| "set_user_lock: not a user resource")?;
            (view.balance(), view.stats(), *view.initial_lock_hash())
        };
        let new_total = USER_HEADER_LEN + new_lock.wire_body_len();
        self.resize(new_total);
        write_user(self.data_mut(), balance, stats, &ilh, new_lock)
    }
}
