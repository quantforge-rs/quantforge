//! The store the server reads and writes, and the turns its requests take
//! at it. Every store call runs on a blocking thread once a turn is free, so
//! that a slow store never stalls the requests that do not need it, and the
//! routes and the job queue share the turns, as many as the store's pool has
//! connections (ADR 0001).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use super::refusal::Refusal;
use crate::ports::{CandleStore, JobStore, SegmentStore};

/// The storage ports the server uses, as one trait object: MSRV 1.85 cannot
/// turn one trait object into another, so this trait stands for all three.
/// Every store implements it.
pub trait ServedStore: CandleStore + SegmentStore + JobStore {}

impl<S: CandleStore + SegmentStore + JobStore> ServedStore for S {}

/// How many calls the server makes on its store at once, and how long one
/// waits for its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLimits {
    /// As many as the store's pool has connections.
    pub concurrency: usize,
    /// How long a call waits for its turn before its request is answered
    /// 503.
    pub wait: Duration,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            concurrency: 8,
            wait: Duration::from_secs(2),
        }
    }
}

/// The server's store and its turns, shared by every route and the job
/// queue.
#[derive(Clone)]
pub struct ServerStore {
    store: Arc<dyn ServedStore>,
    turns: Arc<Semaphore>,
    wait: Duration,
}

impl ServerStore {
    /// The server's handle on `store`. Each of its calls should be bounded,
    /// as a store from `AnyStore::within` is: a call keeps its turn for as
    /// long as it takes, even after its client gives up.
    pub fn new<S: ServedStore + 'static>(store: S, limits: StoreLimits) -> Self {
        Self {
            store: Arc::new(store),
            turns: Arc::new(Semaphore::new(limits.concurrency.max(1))),
            wait: limits.wait,
        }
    }

    /// Runs `work` on a blocking thread once a turn at the store is free, or
    /// refuses as unavailable when none frees up in time. The turn goes
    /// with the work, so a request whose client hangs up keeps it until its
    /// store calls end.
    pub(super) async fn call<T, F>(&self, work: F) -> Result<T, Refusal>
    where
        T: Send + 'static,
        F: FnOnce(&dyn ServedStore) -> Result<T, Refusal> + Send + 'static,
    {
        let turn = tokio::time::timeout(self.wait, Arc::clone(&self.turns).acquire_owned())
            .await
            .map_err(|_| {
                Refusal::unavailable(
                    "the server is answering as many requests as its store can take; retry \
                     later",
                )
            })?
            .map_err(|_| Refusal::internal("the server is shutting down"))?;
        let store = Arc::clone(&self.store);
        let done = tokio::task::spawn_blocking(move || {
            let _turn = turn;
            work(store.as_ref())
        })
        .await;
        done.unwrap_or_else(|_| Err(Refusal::internal("the server failed while answering")))
    }
}

impl fmt::Debug for ServerStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerStore")
            .field("wait", &self.wait)
            .finish_non_exhaustive()
    }
}
