//! Automation on external signals (ADR 0002 §1): what turns an alert into
//! a risk-sized, protected position. It uses [`crate::model`] and
//! [`crate::ports`], as the engines do, and never a candle strategy.
//!
//! Its first part is here: [`policy`], the service an operator's commands
//! keep a profile's policy through, and the one place an entry gets the
//! policy it is admitted under. Admission, the executor and monitoring
//! arrive with their own stories.

pub mod policy;

pub use policy::{AssetSettings, PolicyCheck, PolicyError, PolicyOutcome, PolicyService};
