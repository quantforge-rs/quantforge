//! What every backend promises for a commit, and how long a call may take:
//! the settings behind `docs/storage-durability.md`.

use std::time::Duration;

/// What a store does when its effective settings are weaker than the
/// durability contract. Both modes apply the same settings to every
/// connection; they differ only in what a weaker one meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Durability {
    /// Every connection is read back, and a weaker one is refused as
    /// `StorageError::DurabilityUnmet`: what a store that trades opens with.
    Enforced,
    /// Nothing is refused, and the host reads `StoreControl::durability`
    /// and warns: for commands whose writes can be made again.
    Reported,
}

/// What a route may take in all when nothing sets it: TradingView's 3 s
/// delivery limit, less a second for TLS, authentication and the answer.
pub const DEFAULT_ROUTE_BUDGET: Duration = Duration::from_secs(2);

/// Limits every backend shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StorageLimits {
    pub durability: Durability,
    /// The longest one wait in a storage call may take, for a lock or a free
    /// connection: SQLite's wait for a lock, PostgreSQL's `lock_timeout` and
    /// its pool wait.
    pub wait_budget: Duration,
    /// All that one call of a time-critical route may take, waits and
    /// statements together: what a host hands `AnyStore::within`.
    pub route_budget: Duration,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            durability: Durability::Enforced,
            wait_budget: Duration::from_secs(5),
            route_budget: DEFAULT_ROUTE_BUDGET,
        }
    }
}

/// The part of the `remaining` time of a call that a wait for a lock may
/// take. It ends before the call does, so that a call that runs out of time
/// hears why from the backend, and a held lock is contention rather than an
/// unknown outcome.
pub(crate) fn lock_wait(remaining: Duration) -> Duration {
    remaining / 4 * 3
}
