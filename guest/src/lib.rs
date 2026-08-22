//! The tic-tac-toe guest program for vprogs.
//!
//! Two-module layout over the vprogs batteries:
//! - [`runtime`]: this app's runtime choices: lock/signer variants and dispatchers, kinds, domains,
//!   resource-id derivations, the env-provided genesis key, and the ix wire format with this
//!   program's actions. The app-agnostic machinery (lock/signer traits, auth, auth context,
//!   tx-input parsing, lifecycle, sig-message digest) is imported from vprogs' runtime-processor
//!   lib on branch `guest-batteries` and re-exported there.
//! - [`program`]: this program's logic: accounts (config, deposit, transfer, withdraw) and the
//!   staked tic-tac-toe game (landed next), plus the deposit policy.
//!
//! The dependency direction is one-way: `program` may use `runtime`, never the reverse.
//!
//! The crate also doubles as the shared wire library for host crates (`node`, `driver`) and the
//! web encoder: wire formats and their codecs have one source of truth in this lib.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
    )
)]

extern crate alloc;

pub mod program;
pub mod runtime;
