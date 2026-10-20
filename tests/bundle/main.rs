//! The strategy bundle spec, `docs/bundle-spec.md`, tested from outside the
//! crate:
//! - the shared cases every implementation of the spec passes;
//! - bundles built from real directories, which hash the same whatever the
//!   files' order, times, and modes;
//! - the messages a refused bundle shows;
//! - a canonical tar read by the system's `tar`;
//! - the spec document, held to the code.

mod archive;
mod determinism;
mod docs;
mod fixture;
mod messages;
