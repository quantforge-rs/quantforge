//! The policy store's steps, and the checks coded here: stores saving at
//! once from one revision store one policy a round, whole, and a profile
//! whose first revision cannot be stored is not recorded.

use std::sync::Barrier;
use std::time::{Duration, Instant};

use quantforge::model::futures::{
    OwnerTag, PolicyChange, PolicyRevision, ProfileId, ProfileIdentity, ProfileLabel, ProfilePolicy,
};
use quantforge::model::{AccountRef, Category, Venue};
use quantforge::{
    Environment, PolicyStore, StorageError, StorageErrorKind, StoreControl, VenueAccountId,
};

use super::cases::StampText;
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "policies.saves_racing",
        "stores saving at once from one revision store one policy each round, whole, and the others change nothing",
    ),
    (
        "policies.failed_create_changes_nothing",
        "a profile whose first revision cannot be stored is not recorded, and is created once the store takes the row",
    ),
];

pub fn check<S: PolicyStore + StoreControl>(id: &str) -> Coded<S> {
    match id {
        "policies.saves_racing" => saves_racing,
        "policies.failed_create_changes_nothing" => failed_create_changes_nothing,
        other => panic!("no policy check {other}"),
    }
}

fn profile(text: &str) -> ProfileId {
    ProfileId::new(text).expect("a profile id")
}

/// The policy a step gives as its stored text.
fn given(text: &str) -> ProfilePolicy {
    ProfilePolicy::from_stored(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

/// A read's answer held to the step's: the stored text, nothing, or a
/// refusal.
fn held(
    outcome: Result<Option<ProfilePolicy>, StorageError>,
    expect: &Option<String>,
    refused: &Option<super::Refusal>,
    what: &str,
) {
    match refused {
        Some(refused) => expect_refused(outcome, refused, what),
        None => {
            let found = outcome
                .unwrap_or_else(|error| panic!("{what}: {error}"))
                .map(|policy| policy.to_stored().expect("the stored text"));
            assert_eq!(&found, expect, "{what}");
        }
    }
}

/// Runs a policy store step; `false` for a step of another port.
pub fn step<S: PolicyStore + ?Sized>(store: &S, step: &Step) -> bool {
    match step {
        Step::CreateProfile {
            policy,
            at,
            refused,
        } => {
            let outcome = store.create_profile(&given(policy), *at);
            match refused {
                Some(refused) => expect_refused(outcome, refused, "create a profile"),
                None => outcome.expect("create a profile"),
            }
        }
        Step::Profiles { expect } => {
            let found: Vec<serde_json::Value> = store
                .profiles()
                .expect("the profiles")
                .iter()
                .map(|identity| serde_json::to_value(identity).expect("json"))
                .collect();
            assert_eq!(&found, expect, "the profiles");
        }
        Step::ProfilePolicy {
            profile: id,
            expect,
            refused,
        } => {
            let outcome = store.profile_policy(&profile(id));
            held(outcome, expect, refused, &format!("the policy of {id}"));
        }
        Step::ProfilePolicyAt {
            profile: id,
            revision,
            expect,
            refused,
        } => {
            let at = PolicyRevision::new(*revision).expect("a revision");
            let outcome = store.profile_policy_at(&profile(id), at);
            held(outcome, expect, refused, &format!("revision {at} of {id}"));
        }
        Step::PolicyRevisions {
            profile: id,
            expect,
        } => {
            let found: Vec<StampText> = store
                .policy_revisions(&profile(id))
                .expect("the revisions")
                .iter()
                .map(|stamp| StampText {
                    revision: stamp.revision.get(),
                    changed_at_ms: stamp.changed_at_ms,
                })
                .collect();
            assert_eq!(&found, expect, "the revisions of {id}");
        }
        Step::SavePolicy {
            policy,
            at,
            expect,
            refused,
        } => {
            let outcome = store.save_policy(&given(policy), *at);
            match (refused, expect) {
                (Some(refused), None) => expect_refused(outcome, refused, "save a policy"),
                (None, Some(expect)) => {
                    assert_eq!(&outcome.expect("save a policy"), expect, "stored {policy}");
                }
                other => panic!("a policy is stored, not stored, or refused, not {other:?}"),
            }
        }
        _ => return false,
    }
    true
}

/// A call made again while another writer holds what it needs: contention
/// means nothing changed.
fn past_contention<T>(
    mut call: impl FnMut() -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match call() {
            Err(busy)
                if busy.kind() == StorageErrorKind::Contention && Instant::now() < deadline => {}
            answer => return answer,
        }
    }
}

/// Profile `main` of a demo account, as it is created.
fn created() -> ProfilePolicy {
    let account = VenueAccountId::issued("71002233").expect("an account id");
    ProfilePolicy::new(
        ProfileIdentity::new(
            profile("main"),
            AccountRef::new(Venue::Bybit, Environment::Demo, account).expect("an account"),
            Category::Linear,
            OwnerTag::new("k3x9p2").expect("an owner tag"),
        )
        .expect("an identity"),
    )
}

/// `policy` under a label of the writer's own.
fn labelled(policy: &ProfilePolicy, label: String) -> ProfilePolicy {
    let label = ProfileLabel::new(label).expect("a label");
    policy
        .apply(&PolicyChange::Label(Some(label)))
        .expect("a change the policy takes")
        .expect("a change that changes it")
}

/// Round after round, every writer makes a revision of its own from the
/// one the profile stands at, and all save at once, each through a store
/// of its own. One is stored, and it is that writer's, whole; the others
/// are told nothing changed. No revision is skipped and none made twice.
fn saves_racing<S: PolicyStore + StoreControl>(fixture: &Fixture<S>) {
    const WRITERS: usize = 4;
    const ROUNDS: u64 = 10;
    let store = &fixture.store;
    store.bind_environment(Environment::Demo).expect("bound");
    let mut current = created();
    store.create_profile(&current, 1).expect("a profile");
    let writers: Vec<S> = (0..WRITERS).map(|_| (fixture.reopen)()).collect();
    for round in 0..ROUNDS {
        let candidates: Vec<ProfilePolicy> = (0..WRITERS)
            .map(|writer| labelled(&current, format!("round {round} writer {writer}")))
            .collect();
        let barrier = Barrier::new(WRITERS);
        let stored: Vec<bool> = std::thread::scope(|scope| {
            let saving: Vec<_> = writers
                .iter()
                .zip(&candidates)
                .map(|(writer, candidate)| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        past_contention(|| writer.save_policy(candidate, 2)).expect("a save")
                    })
                })
                .collect();
            saving
                .into_iter()
                .map(|writer| writer.join().expect("a writer"))
                .collect()
        });
        let winners: Vec<usize> = (0..WRITERS).filter(|writer| stored[*writer]).collect();
        let [winner] = winners.as_slice() else {
            panic!("round {round}: {stored:?}");
        };
        let newest = store
            .profile_policy(&profile("main"))
            .expect("a read")
            .expect("the profile");
        assert_eq!(
            newest, candidates[*winner],
            "round {round}: whole, the winner's"
        );
        current = newest;
    }
    let numbers: Vec<u64> = store
        .policy_revisions(&profile("main"))
        .expect("the revisions")
        .iter()
        .map(|stamp| stamp.revision.get())
        .collect();
    assert_eq!(numbers, (1..=1 + ROUNDS).collect::<Vec<u64>>());
}

/// With the revisions table refusing every new row, a profile's creation
/// fails whole: the profile's own row, which its table took, is gone with
/// it. Once the table takes rows again, the same creation goes through.
fn failed_create_changes_nothing<S: PolicyStore + StoreControl>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &fixture.raw);
    store.bind_environment(Environment::Demo).expect("bound");
    let policy = created();
    let count = |table: &str| raw.select(&format!("SELECT CAST(COUNT(*) AS TEXT) FROM {table}"));
    let none = [[Some("0".to_string())]];

    let [refuse, allow] = raw.write_refusal("policy_revisions");
    raw.execute(&refuse);
    store
        .create_profile(&policy, 1)
        .expect_err("the revisions table refuses the row");
    assert_eq!(store.profiles().expect("a read"), []);
    assert_eq!(
        count("automation_profiles"),
        none,
        "no profile without a policy"
    );
    assert_eq!(count("policy_revisions"), none);
    assert_eq!(
        store.profile_policy(&profile("main")).expect("a read"),
        None
    );

    raw.execute(&allow);
    store.create_profile(&policy, 2).expect("the profile");
    assert_eq!(
        store.profile_policy(&profile("main")).expect("a read"),
        Some(policy)
    );
    let stamps = store.policy_revisions(&profile("main")).expect("stamps");
    assert_eq!(stamps.len(), 1);
    assert_eq!(stamps[0].changed_at_ms, 2);
}
