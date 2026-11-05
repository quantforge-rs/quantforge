//! Running a transaction again when the store says that is safe. The
//! adapters themselves never retry (`docs/storage-schema.md`): only a caller
//! knows that all it repeats is the transaction.

use std::thread;
use std::time::{Duration, Instant};

use crate::{StorageError, StorageErrorKind};

const FIRST_PAUSE: Duration = Duration::from_millis(10);
const LONGEST_PAUSE: Duration = Duration::from_millis(200);

/// Runs `transaction` until the store stops answering
/// [`StorageError::Conflict`], or `budget` runs out, and returns its last
/// answer. Nothing else is retried: a conflict alone says nothing was
/// changed, and [`StorageError::OutcomeUnknown`] above all ends it, so a
/// write that may have committed is never made twice.
///
/// `transaction` is given what is left of the budget, for the store calls
/// inside it: `|left| store.within(left).upsert_candles(market, candles)`.
/// It holds storage calls and nothing else. An exchange request inside it
/// would be sent again, so one never belongs here.
pub fn retry_conflicts<T>(
    budget: Duration,
    mut transaction: impl FnMut(Duration) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let deadline = Instant::now() + budget;
    let mut left = budget;
    let mut pause = FIRST_PAUSE;
    loop {
        match transaction(left) {
            Err(conflict) if conflict.kind() == StorageErrorKind::Conflict => {
                left = deadline.saturating_duration_since(Instant::now());
                if left <= pause {
                    return Err(conflict);
                }
                thread::sleep(pause);
                left -= pause;
                pause = (pause * 2).min(LONGEST_PAUSE);
            }
            answer => return answer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict() -> StorageError {
        StorageError::Conflict("lost".to_string())
    }

    #[test]
    fn a_conflict_is_run_again_until_the_transaction_goes_through() {
        let mut attempts = 0;
        let answer = retry_conflicts(Duration::from_secs(5), |_| {
            attempts += 1;
            if attempts < 3 { Err(conflict()) } else { Ok(7) }
        });
        assert_eq!(answer.expect("the third attempt"), 7);
        assert_eq!(attempts, 3);
    }

    #[test]
    fn every_other_failure_ends_it_at_once() {
        let failures: [fn() -> StorageError; 4] = [
            || StorageError::OutcomeUnknown("lost its answer".to_string()),
            || StorageError::Contention("held".to_string()),
            || StorageError::Unavailable("gone".to_string()),
            || StorageError::InvalidArgument("refused".to_string()),
        ];
        for failure in failures {
            let mut attempts = 0;
            let answer: Result<(), _> = retry_conflicts(Duration::from_secs(5), |_| {
                attempts += 1;
                Err(failure())
            });
            let refused = answer.expect_err("a failure");
            assert_eq!(attempts, 1, "{refused}");
            assert_eq!(refused.kind(), failure().kind());
        }
    }

    #[test]
    fn it_stops_inside_the_budget_and_answers_the_conflict() {
        let budget = Duration::from_millis(100);
        let started = Instant::now();
        let mut attempts = 0;
        let answer: Result<(), _> = retry_conflicts(budget, |_| {
            attempts += 1;
            Err(conflict())
        });
        assert_eq!(
            answer.expect_err("never through").kind(),
            StorageErrorKind::Conflict
        );
        // A timer may wake late on a loaded machine, and never by this much.
        let took = started.elapsed();
        assert!(took < budget + Duration::from_secs(1), "{took:?}");
        assert!(attempts > 1, "{attempts}");
    }

    #[test]
    fn each_attempt_is_handed_what_is_left() {
        let budget = Duration::from_secs(5);
        let mut lefts = Vec::new();
        let answer = retry_conflicts(budget, |left| {
            lefts.push(left);
            if lefts.len() < 3 {
                Err(conflict())
            } else {
                Ok(())
            }
        });
        answer.expect("through");
        assert_eq!(lefts[0], budget);
        assert!(lefts.windows(2).all(|pair| pair[1] < pair[0]), "{lefts:?}");
    }
}
