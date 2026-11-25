//! The strategy bundle spec, `docs/bundle-spec.md`, tested from outside the
//! crate:
//! - the shared cases every implementation of the spec passes;
//! - bundles built from real directories, which hash the same whatever the
//!   files' order, times, and modes;
//! - the messages a refused bundle shows;
//! - a canonical tar read by the system's `tar`;
//! - the spec document, held to the code;
//! - bundles made to do harm, and broken ones: every one refused, and
//!   nothing a reader accepts naming a place outside the bundle;
//! - this build's engine version, which a bundle's requirement is held to.

mod archive;
mod determinism;
mod docs;
mod fixture;
mod hostile;
mod messages;

use quantforge::ENGINE_VERSION;
use quantforge::protocol::bundle::{EngineRequirement, EngineVersion};

/// A server holds a bundle's `engine` requirement to its own version, and
/// the Python package reports that version as the engine it was built with:
/// it is the crate's own, and one the spec can spell.
#[test]
fn this_builds_engine_version_is_the_crates_and_one_a_bundle_can_require() {
    assert_eq!(ENGINE_VERSION, env!("CARGO_PKG_VERSION"));
    let version: EngineVersion = ENGINE_VERSION.parse().expect("an engine version");
    assert_eq!(version.to_string(), ENGINE_VERSION);
    let requirement: EngineRequirement = format!("=={ENGINE_VERSION}")
        .parse()
        .expect("a requirement");
    assert!(requirement.matches(version));
}
