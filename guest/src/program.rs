//! Tic-tac-toe program logic: this app's resources, kinds and actions.
//!
//! Program-owned decisions live here: the resource kinds (config, user, game) and their
//! per-kind domain tags / id derivations (`kind`, `domain`, `resource_id`), the resource wire
//! formats (`config`, `user`, later `game`) with the user resource carrying the game counters
//! the game derivation and stats need, and the actions (wire types, decode, apply) in
//! [`action`]. The account model is the starting point; the staked game lands on top.
//!
//! `program` may use [`crate::runtime`] and the vprogs batteries, never the reverse.

pub mod action;
pub mod config;
pub mod domain;
pub mod kind;
pub mod resource_ext;
pub mod resource_id;
pub mod run;
pub mod user;
