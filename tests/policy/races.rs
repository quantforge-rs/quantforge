//! Two changes made from one revision, on each backend. Stepped by hand,
//! from the fixture: the first stored is the next revision, the second
//! changes nothing and is made again on top of it, and either order ends
//! where the independent model says. Then for real: stores opened by as
//! many writers as race, each change stored once, in some order, with no
//! revision skipped or made twice.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use quantforge::automation::{PolicyError, PolicyOutcome, PolicyService};
use quantforge::model::futures::{PolicyChange, PolicyRevision, ProfileId, ProfilePolicy};
use quantforge::model::{Asset, Money};
use quantforge::{PolicyStore, StorageErrorKind};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::cases::{Fixture, Line, Race, change, fixture};
use crate::support::{Backend, T, Ticking, on_each_backend, stored};

fn profile() -> ProfileId {
    ProfileId::new("main").expect("a profile id")
}

/// The newest policy the store holds, which must be there.
fn newest(store: &impl PolicyStore) -> ProfilePolicy {
    store
        .profile_policy(&profile())
        .expect("a read")
        .expect("the profile")
}

/// `policy` after `asked`, which must change it.
fn after(policy: &ProfilePolicy, line: &Line, asked: &Value) -> ProfilePolicy {
    policy
        .apply(&change(line, asked))
        .expect("a change the policy takes")
        .expect("a change that changes it")
}

/// Creates the race's profile and brings it to the revision both changes
/// start from.
fn based(store: &impl PolicyStore, line: &Line, race: &Race) -> ProfilePolicy {
    let mut policy = ProfilePolicy::new(line.identity.clone());
    store.create_profile(&policy, T).expect("a profile");
    for (n, asked) in (1..).zip(&race.base) {
        policy = after(&policy, line, asked);
        assert!(store.save_policy(&policy, T + n).expect("a save"));
    }
    assert_eq!(stored(&newest(store)), race.base_document, "{}", race.id);
    policy
}

/// One order of a race, stepped: `first` is stored, `second`, made from
/// the same revision, is not, and made again it ends at `both`.
fn stepped(
    store: &impl PolicyStore,
    line: &Line,
    race: &Race,
    order: [&Value; 2],
    ends: [&str; 2],
) {
    let base = based(store, line, race);
    let [first, second] = order.map(|asked| after(&base, line, asked));
    assert_eq!(first.revision(), second.revision(), "{}", race.id);

    assert!(store.save_policy(&first, T + 100).expect("a save"));
    // Made from a revision the profile has left: nothing is written.
    assert!(!store.save_policy(&second, T + 101).expect("a save"));
    assert_eq!(stored(&newest(store)), ends[0], "{}", race.id);
    // Stored again, the first is no revision either.
    assert!(!store.save_policy(&first, T + 102).expect("a save"));

    // Read again and made again, it is the revision after.
    let again = after(&newest(store), line, order[1]);
    assert!(store.save_policy(&again, T + 103).expect("a save"));
    assert_eq!(stored(&newest(store)), ends[1], "{}", race.id);

    // A revision is never skipped: one made two changes ahead is not stored.
    let next = after(&again, line, &json!({"label": "one"}));
    let ahead = after(&next, line, &json!({"label": "two"}));
    assert!(!store.save_policy(&ahead, T + 104).expect("a save"));
    let stamps = store.policy_revisions(&profile()).expect("stamps");
    let last = u64::try_from(stamps.len()).expect("a count");
    assert_eq!(last, again.revision().get(), "{}", race.id);
    assert_eq!(
        stamps.last().map(|stamp| stamp.changed_at_ms),
        Some(T + 103)
    );
}

async fn stepped_races<B: Backend>(backend: impl Fn() -> Option<B>, fixture: &Fixture) {
    assert_eq!(fixture.races.len(), 4, "one race for each product line");
    for race in &fixture.races {
        let line = fixture.line(&race.line);
        let orders = [
            (
                [&race.left, &race.right],
                [&race.left_alone, &race.left_then_right],
            ),
            (
                [&race.right, &race.left],
                [&race.right_alone, &race.right_then_left],
            ),
        ];
        for (order, ends) in orders {
            let Some(backend) = backend() else { return };
            stepped(&backend.open(), line, race, order, ends.map(String::as_str));
        }
    }
}

mod of_two_changes_from_one_revision_one_is_stored_and_the_other_is_made_again {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sqlite() {
        stepped_races(|| Some(crate::support::Sqlite::new()), &fixture()).await;
    }

    #[cfg(feature = "postgres")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn postgres() {
        stepped_races(crate::support::postgres::Postgres::new, &fixture()).await;
    }
}

/// How many writers race.
const WRITERS: u32 = 8;

/// A change asked again while another writer holds the store. Contention
/// means nothing changed, and no store promises every writer a turn within
/// one wait: on a slow disk the others can outlast it. A store that never
/// lets go still fails the test.
fn past_contention(
    mut change: impl FnMut() -> Result<PolicyOutcome, PolicyError>,
) -> Result<PolicyOutcome, PolicyError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match change() {
            Err(PolicyError::Storage(busy))
                if busy.kind() == StorageErrorKind::Contention && Instant::now() < deadline => {}
            answer => return answer,
        }
    }
}

/// The change writer `n` makes: capital in an asset of its own.
fn capital_of(n: u32) -> PolicyChange {
    let asset = Asset::new(format!("COIN{n}")).expect("an asset");
    PolicyChange::SetCapital(Money::new(Decimal::from(n + 1), asset))
}

/// Writers that name no revision are each stored, one revision apiece.
async fn writers_racing_on_one_profile_each_make_one_revision<B: Backend>(backend: &B) {
    let fixture = fixture();
    let line = fixture.line("bybit inverse");
    let store = backend.open();
    store
        .create_profile(&ProfilePolicy::new(line.identity.clone()), T)
        .expect("a profile");

    let made: Vec<u64> = std::thread::scope(|scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|n| {
                // Each writer a store of its own, as another process has.
                let store = backend.open();
                scope.spawn(move || {
                    let clock = Ticking::default();
                    let service = PolicyService::new(&store, &clock);
                    let outcome =
                        past_contention(|| service.change(&profile(), None, &capital_of(n)))
                            .unwrap_or_else(|error| panic!("writer {n}: {error}"));
                    assert!(outcome.changed, "writer {n}");
                    outcome.policy.revision().get()
                })
            })
            .collect();
        writers
            .into_iter()
            .map(|writer| writer.join().expect("a writer"))
            .collect()
    });

    // Every writer got a revision of its own: 2 to 9, each once.
    let revisions: BTreeSet<u64> = made.iter().copied().collect();
    let expected: BTreeSet<u64> = (2..2 + u64::from(WRITERS)).collect();
    assert_eq!(revisions, expected, "{made:?}");
    let stamps = store.policy_revisions(&profile()).expect("stamps");
    let numbers: Vec<u64> = stamps.iter().map(|stamp| stamp.revision.get()).collect();
    assert_eq!(numbers, (1..=1 + u64::from(WRITERS)).collect::<Vec<u64>>());
    // And each revision holds one change more than the one before: none
    // was lost under another.
    for (number, held) in (1..).zip(0..=WRITERS) {
        let revision = PolicyRevision::new(number).expect("a revision");
        let at = store
            .profile_policy_at(&profile(), revision)
            .expect("a read")
            .expect("the revision");
        assert_eq!(at.capital().len(), held as usize, "revision {number}");
    }
    assert_eq!(newest(&store).capital().len(), WRITERS as usize);
}

/// Of writers that all name the same revision, one is stored and the
/// others are told the profile moved on.
async fn of_writers_naming_one_revision_one_is_stored<B: Backend>(backend: &B) {
    let fixture = fixture();
    let line = fixture.line("binance linear");
    let store = backend.open();
    store
        .create_profile(&ProfilePolicy::new(line.identity.clone()), T)
        .expect("a profile");

    let outcomes: Vec<Result<u64, String>> = std::thread::scope(|scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|n| {
                let store = backend.open();
                scope.spawn(move || {
                    let clock = Ticking::default();
                    let service = PolicyService::new(&store, &clock);
                    let named = Some(PolicyRevision::FIRST);
                    match past_contention(|| service.change(&profile(), named, &capital_of(n))) {
                        Ok(outcome) => Ok(outcome.policy.revision().get()),
                        Err(PolicyError::Moved { found, .. }) => Err(found.to_string()),
                        Err(other) => panic!("writer {n}: {other}"),
                    }
                })
            })
            .collect();
        writers
            .into_iter()
            .map(|writer| writer.join().expect("a writer"))
            .collect()
    });
    let stored: Vec<&u64> = outcomes
        .iter()
        .filter_map(|made| made.as_ref().ok())
        .collect();
    assert_eq!(stored, [&2], "{outcomes:?}");
    assert!(
        outcomes
            .iter()
            .filter_map(|made| made.as_ref().err())
            .all(|found| found == "2"),
        "{outcomes:?}"
    );
    let held = newest(&store);
    assert_eq!(held.revision().get(), 2);
    assert_eq!(held.capital().len(), 1, "one change, whole");
    assert_eq!(store.policy_revisions(&profile()).expect("stamps").len(), 2);
}

on_each_backend!(
    writers_racing_on_one_profile_each_make_one_revision,
    of_writers_naming_one_revision_one_is_stored,
);
