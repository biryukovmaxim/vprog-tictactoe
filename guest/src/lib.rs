//! The tic-tac-toe guest program for vprogs.
//!
//! Two-module layout, pending the vprogs runtime-processor split:
//! - `runtime`: framework ported from vprogs' runtime-processor (resource IDs, locks, lifecycle, ix
//!   wire format, deposit policy).
//! - `program`: this program's logic: accounts (config, deposit, transfer, withdraw) and the staked
//!   tic-tac-toe game.
//!
//! The dependency direction is one-way: `program` may use `runtime`, never the reverse. Skeleton
//! only; both modules land with the guest milestone.

#![no_std]
