//! Closure-based combinators on `Resource<'a>` that layer typed views onto
//! the byte-backed storage.
//!
//! Every mutating combinator requires the tx to have declared the resource `AccessType::Write`:
//! nothing on the guest path enforces the declared access mode, so this module is the single
//! choke point through which all action writes pass. Read-only (`view_*`) combinators accept
//! both modes.

use vprogs_core_types::{AccessType, ResourceId};
use vprogs_zk_abi::transaction_processor::Resource;

use crate::{
    program::resources::{
        config::{CONFIG_HEADER_LEN, ConfigView, ConfigViewMut, config_total_len, write_config},
        game::{Cell, GAME_WIRE_LEN, GameView, GameViewMut, write_game},
        kind::{KIND_CONFIG, KIND_GAME, KIND_USER, kind_of},
        user::{GameStats, USER_HEADER_LEN, UserView, UserViewMut, user_total_len, write_user},
    },
    runtime::lock::LockEnum,
};

/// Whether the tx declared this resource writable; the gate every mutating combinator checks.
fn is_writable(r: &Resource<'_>) -> bool {
    r.access_type() == AccessType::Write
}

/// Extension trait over `Resource<'a>`; see module docs.
pub trait ResourceExt<'a> {
    fn kind(&self) -> Option<u8>;

    fn view_config<R>(&self, f: impl FnOnce(ConfigView<'_>) -> R) -> Option<R>;
    fn view_user<R>(&self, f: impl FnOnce(UserView<'_>) -> R) -> Option<R>;
    fn view_game<R>(&self, f: impl FnOnce(GameView<'_>) -> R) -> Option<R>;

    fn modify_config<R>(&mut self, f: impl FnOnce(&mut ConfigViewMut<'_>) -> R) -> Option<R>;
    fn modify_user<R>(&mut self, f: impl FnOnce(&mut UserViewMut<'_>) -> R) -> Option<R>;
    fn modify_game<R>(&mut self, f: impl FnOnce(&mut GameViewMut<'_>) -> R) -> Option<R>;

    fn init_config(
        &mut self,
        min_withdrawal_amount: u64,
        turn_ttl: u64,
        covenant_id: &[u8; 32],
        lock: &LockEnum<'_>,
    ) -> Result<(), &'static str>;
    fn init_user(
        &mut self,
        balance: u64,
        initial_lock_hash: &[u8; 32],
        lock: &LockEnum<'_>,
    ) -> Result<(), &'static str>;
    fn init_game(
        &mut self,
        creator: &ResourceId,
        creator_mark: Cell,
        stake: u64,
        rounds_total: u8,
    ) -> Result<(), &'static str>;

    fn set_config_lock(&mut self, new_lock: &LockEnum<'_>) -> Result<(), &'static str>;
    fn set_user_lock(&mut self, new_lock: &LockEnum<'_>) -> Result<(), &'static str>;
}

impl<'a> ResourceExt<'a> for Resource<'a> {
    fn kind(&self) -> Option<u8> {
        // Empty data means the slot holds no live resource: a `New` slot has nothing yet and a
        // `Deleted` one was emptied on teardown. Reading both off data-emptiness keeps this view
        // layer lifecycle-agnostic (the lifecycle lives in `ApplyContext`) while still rejecting
        // use-after-delete: `view_*`/`modify_*` return `None` on an emptied slot.
        if self.data().is_empty() {
            return None;
        }
        kind_of(self.data())
    }

    fn view_config<R>(&self, f: impl FnOnce(ConfigView<'_>) -> R) -> Option<R> {
        if self.kind()? != KIND_CONFIG {
            return None;
        }
        ConfigView::from_bytes(self.data()).ok().map(f)
    }

    fn view_user<R>(&self, f: impl FnOnce(UserView<'_>) -> R) -> Option<R> {
        if self.kind()? != KIND_USER {
            return None;
        }
        UserView::from_bytes(self.data()).ok().map(f)
    }

    fn view_game<R>(&self, f: impl FnOnce(GameView<'_>) -> R) -> Option<R> {
        if self.kind()? != KIND_GAME {
            return None;
        }
        GameView::from_bytes(self.data()).ok().map(f)
    }

    fn modify_config<R>(&mut self, f: impl FnOnce(&mut ConfigViewMut<'_>) -> R) -> Option<R> {
        if self.kind()? != KIND_CONFIG || !is_writable(self) {
            return None;
        }
        let mut mv = ConfigViewMut::from_bytes_mut(self.data_mut()).ok()?;
        Some(f(&mut mv))
    }

    fn modify_user<R>(&mut self, f: impl FnOnce(&mut UserViewMut<'_>) -> R) -> Option<R> {
        if self.kind()? != KIND_USER || !is_writable(self) {
            return None;
        }
        let mut mv = UserViewMut::from_bytes_mut(self.data_mut()).ok()?;
        Some(f(&mut mv))
    }

    fn modify_game<R>(&mut self, f: impl FnOnce(&mut GameViewMut<'_>) -> R) -> Option<R> {
        if self.kind()? != KIND_GAME || !is_writable(self) {
            return None;
        }
        let mut mv = GameViewMut::from_bytes_mut(self.data_mut()).ok()?;
        Some(f(&mut mv))
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
        if self.kind() != Some(KIND_CONFIG) {
            return Err("set_config_lock: not a config resource");
        }
        if !is_writable(self) {
            return Err("set_config_lock: resource not declared writable");
        }
        // Snapshot fixed-header fields before resize (which would invalidate
        // the existing data slice).
        let (min_withdrawal_amount, turn_ttl, covenant_id) = {
            let view = ConfigView::from_bytes(self.data())?;
            (view.min_withdrawal_amount(), view.turn_ttl(), *view.covenant_id())
        };
        let new_total = CONFIG_HEADER_LEN + new_lock.wire_body_len();
        self.resize(new_total);
        write_config(self.data_mut(), min_withdrawal_amount, turn_ttl, &covenant_id, new_lock)
    }

    fn set_user_lock(&mut self, new_lock: &LockEnum<'_>) -> Result<(), &'static str> {
        if self.kind() != Some(KIND_USER) {
            return Err("set_user_lock: not a user resource");
        }
        if !is_writable(self) {
            return Err("set_user_lock: resource not declared writable");
        }
        // Snapshot the fixed fields (balance + game counters) before resize; the rotation
        // must not reset game history.
        let (balance, stats, ilh) = {
            let view = UserView::from_bytes(self.data())?;
            (view.balance(), view.stats(), *view.initial_lock_hash())
        };
        let new_total = USER_HEADER_LEN + new_lock.wire_body_len();
        self.resize(new_total);
        write_user(self.data_mut(), balance, stats, &ilh, new_lock)
    }
}
