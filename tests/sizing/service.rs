//! Sizing under a policy a store hands back, on each backend: every shared
//! case set up through the policy service as an operator would, read by a
//! process that starts on the store, and sized to the same answer the pure
//! model gives. Then an admitted entry, which is sized from the snapshot
//! it was admitted under whatever the profile's policy has become since.

use quantforge::automation::{PolicyError, PolicyService};
use quantforge::model::futures::{
    AssetEdit, Edit, EntryIntent, ExternalIntent, IntentKind, PolicyChange, PositionKey, ProfileId,
    RiskPercent, SignalId,
};
use quantforge::model::{Asset, Leverage, Money, Refusal};
use serde_json::json;

use crate::cases::{Given, answered, decimal, fixture, refused_by_policy};
use crate::support::{Backend, T, Ticking, on_each_backend};

/// The profile of the fixture's `number`th case: one store holds them all.
fn profile_of(number: usize) -> ProfileId {
    ProfileId::new(format!("case-{number:03}")).expect("a profile id")
}

/// Every case, set up through the service and sized from what a store
/// opened anew answers: the same snapshot the model takes, and the same
/// answer, on SQLite and on PostgreSQL.
async fn every_case_sizes_the_same_under_a_policy_read_from_the_store<B: Backend>(backend: &B) {
    let fixture = fixture();
    let clock = Ticking::default();
    {
        // The operator's session: a profile for each case, its capital,
        // and its asset with settings of its own.
        let store = backend.open();
        let service = PolicyService::new(&store, &clock);
        for (number, case) in fixture.sizing.iter().enumerate() {
            let identity = &case.inputs.profile;
            let id = profile_of(number);
            service
                .create_profile(
                    id.clone(),
                    identity.account_ref().clone(),
                    identity.category(),
                )
                .unwrap_or_else(|error| panic!("{}: {error}", case.id));
            for change in case.changes(&case.admitted_on(&fixture)) {
                let made = service
                    .change(&id, None, &change)
                    .unwrap_or_else(|error| panic!("{}: {error}", case.id));
                assert!(made.changed, "{}", case.id);
            }
        }
    }

    // The executor's, on a store opened anew.
    let store = backend.open();
    let service = PolicyService::new(&store, &clock);
    let mut wrong = Vec::new();
    for (number, case) in fixture.sizing.iter().enumerate() {
        let id = profile_of(number);
        let admitted_on = case.admitted_on(&fixture);
        let given = Given::of(&fixture, &case.inputs);
        let got = match service.entry(&id, &admitted_on) {
            Err(PolicyError::Refused(refusal)) => {
                assert!(case.inputs.snapshot.is_none(), "{}: {refusal}", case.id);
                refused_by_policy(&given.instrument.id().settle, refusal.refusal.as_str())
            }
            Err(other) => panic!("{}: {other}", case.id),
            Ok(snapshot) => {
                // The model's snapshot, under this case's profile name.
                let mut expected = case.inputs.snapshot.clone().expect("a snapshot");
                expected["profile"] = json!(id.as_str());
                let stored = serde_json::to_value(&snapshot).expect("a snapshot");
                assert_eq!(stored, expected, "{}", case.id);
                answered(&case.inputs, &given, &snapshot).0
            }
        };
        if got != case.result {
            wrong.push(format!(
                "{}:\n  sized    {got}\n  recorded {}",
                case.id, case.result
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} cases differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// An admitted entry is sized from the snapshot it was admitted under. The
/// operator doubles the risk, the leverage and the capital, then disables
/// the asset; the entry, read back as it was stored, still sizes to what
/// it would have, and only an entry admitted afterwards sees the change.
async fn an_admitted_entry_is_sized_from_its_snapshot_whatever_the_policy_becomes<B: Backend>(
    backend: &B,
) {
    let fixture = fixture();
    let clock = Ticking::default();
    // 1 % of 15,000 USDT on Bybit's SOLUSDT: 4.3 SOL at 5x.
    let case = fixture.case("prd-p9-bybit-linear-long");
    let listed = case.admitted_on(&fixture);
    let given = Given::of(&fixture, &case.inputs);
    let identity = &case.inputs.profile;
    let main = identity.id().clone();
    let symbol = listed.id().symbol.clone();

    let stored_intent = {
        let store = backend.open();
        let service = PolicyService::new(&store, &clock);
        service
            .create_profile(
                main.clone(),
                identity.account_ref().clone(),
                identity.category(),
            )
            .expect("a profile");
        for change in case.changes(&listed) {
            service.change(&main, None, &change).expect("a change");
        }
        let snapshot = service.entry(&main, &listed).expect("a snapshot");
        let key = PositionKey::new(identity.account_ref().clone(), listed.id().clone())
            .expect("a position key");
        let mut intent = ExternalIntent::new(IntentKind::Entry(EntryIntent {
            profile: main.clone(),
            signal: SignalId::new("entry-1").expect("a signal id"),
            signal_time_ms: T,
            key,
            side: case.inputs.side,
            expected_price: case.inputs.expected_price,
            stop: case.inputs.signal_stop,
        }));
        intent.admit(Some(snapshot)).expect("admitted");
        let policy = intent.policy().expect("its policy");
        assert_eq!(answered(&case.inputs, &given, policy).0, case.result);

        let edit = |edit: AssetEdit| PolicyChange::EditAsset {
            symbol: symbol.clone(),
            edit,
        };
        let usdt = Asset::new("USDT").expect("an asset");
        for change in [
            edit(AssetEdit {
                risk_percent: Edit::Set(RiskPercent::new(decimal("2")).expect("a risk")),
                leverage: Edit::Set(Leverage::new(decimal("10")).expect("a leverage")),
                ..AssetEdit::default()
            }),
            PolicyChange::SetCapital(Money::new(decimal("30000"), usdt)),
            edit(AssetEdit {
                enabled: Some(false),
                ..AssetEdit::default()
            }),
        ] {
            let made = service.change(&main, None, &change).expect("a change");
            assert!(made.changed);
        }
        serde_json::to_string(&intent).expect("an intent as stored")
    };

    // A process that starts on the store, with the intent as it was kept.
    let store = backend.open();
    let service = PolicyService::new(&store, &clock);
    let intent: ExternalIntent = serde_json::from_str(&stored_intent).expect("the intent");
    let policy = intent.policy().expect("its policy");
    assert_eq!(
        answered(&case.inputs, &given, policy).0,
        case.result,
        "an admitted entry is sized as it was admitted"
    );

    // No new entry while the asset is disabled; once it is enabled again,
    // one is sized from the settings as they stand: 2 % of 30,000 USDT is
    // 600, which is 17.5 SOL at a 34.23 stop distance, and a tenth of its
    // 4,099.025 USDT is its margin.
    match service.entry(&main, &listed) {
        Err(PolicyError::Refused(refusal)) => assert_eq!(refusal.refusal, Refusal::NotArmed),
        other => panic!("a disabled asset takes no entry: {other:?}"),
    }
    let enable = PolicyChange::EditAsset {
        symbol,
        edit: AssetEdit {
            enabled: Some(true),
            ..AssetEdit::default()
        },
    };
    service.change(&main, None, &enable).expect("enabled");
    let later = service.entry(&main, &listed).expect("a snapshot");
    assert!(later.revision() > policy.revision());
    let (sized, _) = answered(&case.inputs, &given, &later);
    assert_eq!(sized["status"], "sized");
    assert_eq!(sized["budget"], "600");
    assert_eq!(sized["qty"], "17.5");
    assert_eq!(sized["notional"], "4099.025");
    assert_eq!(sized["initial_margin"], "409.9025");
}

on_each_backend!(
    every_case_sizes_the_same_under_a_policy_read_from_the_store,
    an_admitted_entry_is_sized_from_its_snapshot_whatever_the_policy_becomes,
);
