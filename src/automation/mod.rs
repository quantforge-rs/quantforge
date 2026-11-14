//! Automation on external signals (ADR 0002 §1): what turns an alert into
//! a risk-sized, protected position. It uses [`crate::model`] and
//! [`crate::ports`], as the engines do, and never a candle strategy.
//!
//! Two parts are here:
//! - [`policy`], the service an operator's commands keep a profile's policy
//!   through, which also gives an entry on a known instrument the policy it
//!   is admitted under;
//! - [`signal`], the service that resolves an alert's signal for the
//!   profile its webhook URL selected: whose it is, how old, what its
//!   ticker names, and, for an entry, that policy.
//!
//! The store that records a signal, the executor and monitoring arrive
//! with their own stories.

pub mod policy;
pub mod signal;

pub use policy::{AssetSettings, PolicyCheck, PolicyError, PolicyOutcome, PolicyService};
pub use signal::{SignalError, SignalService};
