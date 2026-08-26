//! The app's resources: wire formats, typed views, and the id/kind machinery around
//! them.
//!
//! - [`config`], [`user`], [`game`]: the three resource payloads with their zerocopy raw layouts
//!   and read/write views.
//! - [`kind`]: the first-byte discriminator partition and its typed enums.
//! - [`domain`]: one-byte hash domains, one per kind, so the id derivations cannot collide across
//!   keyspaces.
//! - [`id`]: the domain-separated resource id derivations.
//! - [`ext`]: closure combinators layering the typed views onto `Resource<'a>`.
pub mod config;
pub mod domain;
pub mod ext;
pub mod game;
pub mod id;
pub mod kind;
pub mod user;
