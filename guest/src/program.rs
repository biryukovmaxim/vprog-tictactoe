//! Tic-tac-toe program logic: the app's resources and the actions over them.
//!
//! Program-owned decisions live here: the resources (payloads, views, kinds, domains,
//! id derivations) in [`resources`], the actions (wire types, decode, apply) in
//! [`action`], the deposit policy in [`deposit_policy`], and the thin dispatch loop in
//! [`run`]. The account model is the starting point; the staked game lands on top.
//!
//! `program` may use [`crate::runtime`] and the vprogs batteries, never the reverse.

pub mod action;
pub mod deposit_policy;
pub mod resources;
pub mod run;
