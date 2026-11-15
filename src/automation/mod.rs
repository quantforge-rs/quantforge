//! Automation on external signals (ADR 0002 §1): what turns an alert into
//! a risk-sized, protected position. It uses [`crate::model`] and
//! [`crate::ports`], as the engines do, and never a candle strategy.
//!
//! Three parts are here:
//! - [`policy`], the service an operator's commands keep a profile's policy
//!   through, which also gives an entry on a known instrument the policy it
//!   is admitted under;
//! - [`signal`], the service that resolves an alert's signal for the
//!   profile its webhook URL selected: whose it is, how old, what its
//!   ticker names, and, for an entry, that policy;
//! - [`ledger`], what admission and an executor keep a signal's progress
//!   through: a delivery recorded with what became of it, an order's
//!   identity recorded before it is sent, the venue's evidence that
//!   settles it, and the query that resumes an intent after a restart.
//!
//! The webhook route, the executor and monitoring arrive with their own
//! stories.

pub mod ledger;
pub mod policy;
pub mod signal;

pub use ledger::{Admission, Evidence, ExecutionLedger, LedgerError, Resumed};
pub use policy::{AssetSettings, PolicyCheck, PolicyError, PolicyOutcome, PolicyService};
pub use signal::{SignalError, SignalService};
