//! Leases and activation, decided: what each row and standing comes to.

use super::*;
use crate::model::{AccountRef, Asset, ExchangeId, InstrumentId, Symbol, Venue, VenueAccountId};

fn key() -> PositionKey {
    let id = VenueAccountId::issued("71002233").expect("an account id");
    let account = AccountRef::new(Venue::Bybit, Environment::Demo, id).expect("an account");
    let instrument = InstrumentId::new(
        ExchangeId::BybitLinear,
        Symbol::new("SOLUSDT").expect("a symbol"),
        Asset::new("USDT").expect("an asset"),
    );
    PositionKey::new(account, instrument).expect("a key")
}

fn standing(activation: StoreActivation, epoch: u32) -> StoreStanding {
    StoreStanding {
        store_id: StoreId::new("0123456789abcdef0123456789abcdef").expect("an id"),
        environment: Some(Environment::Demo),
        activation,
        epoch: ActivationEpoch(epoch),
    }
}

fn request(holder: &str, now_ms: TimestampMs) -> LeaseRequest {
    LeaseRequest {
        key: key(),
        profile: ProfileId::new("main").expect("a profile id"),
        holder: LeaseHolder::new(holder).expect("a holder"),
        ttl_ms: 30_000,
        now_ms,
    }
}

fn granted(answer: LeaseAnswer) -> Lease {
    match answer {
        LeaseAnswer::Granted(lease) => lease,
        LeaseAnswer::Held(lease) => panic!("held by {}", lease.holder),
    }
}

#[test]
fn only_an_active_store_grants_a_lease_and_a_lease_fences_at_its_own_epoch() {
    let active = standing(StoreActivation::Active, 1);
    for idle in [StoreActivation::Inactive, StoreActivation::Deactivated] {
        let refused =
            acquire(&request("a", 1_000), None, &standing(idle, 0)).expect_err("not active");
        assert!(refused.to_string().contains("only an active store"));
    }
    let lease = granted(acquire(&request("a", 1_000), None, &active).expect("granted"));
    let row = LeaseColumns::holding(&lease).row(&key()).expect("a row");
    assert_eq!(row.lease.as_ref(), Some(&lease));
    let fence = lease.fence();
    assert_eq!(fenced(&fence, Some(&row), &active), None);

    // Deactivated, or activated again since: every lease of the earlier
    // epoch is void at once.
    for (activation, epoch) in [
        (StoreActivation::Deactivated, 1),
        (StoreActivation::Active, 2),
    ] {
        let now = standing(activation, epoch);
        assert_eq!(
            fenced(&fence, Some(&row), &now),
            Some(Fenced::StoreNotActive {
                activation,
                epoch: ActivationEpoch(epoch)
            })
        );
        assert_eq!(
            renew(&fence, Some(&row), &now, 30_000, 2_000).expect("asked"),
            None
        );
    }
    assert_eq!(fenced(&fence, None, &active), Some(Fenced::NotHeld));
}

#[test]
fn a_released_lease_keeps_its_token_so_the_next_grant_takes_the_one_after() {
    let active = standing(StoreActivation::Active, 1);
    let first = granted(acquire(&request("a", 1_000), None, &active).expect("granted"));
    let held = LeaseColumns::holding(&first).row(&key()).expect("a row");
    assert!(releases(&first.fence(), Some(&held)));
    let released = LeaseColumns::released(held.last_fence)
        .row(&key())
        .expect("a row");
    assert_eq!((released.last_fence, &released.lease), (1, &None));
    assert!(!releases(&first.fence(), Some(&released)), "released once");
    assert_eq!(
        fenced(&first.fence(), Some(&released), &active),
        Some(Fenced::NotHeld)
    );
    let next = granted(acquire(&request("b", 2_000), Some(&released), &active).expect("ok"));
    assert_eq!(next.fence.get(), 2);
    let taken = LeaseColumns::holding(&next).row(&key()).expect("a row");
    assert_eq!(
        fenced(&first.fence(), Some(&taken), &active),
        Some(Fenced::Lost {
            holder: next.holder.clone(),
            token: next.fence
        })
    );
    // A renewal extends the lease and keeps its token and its start.
    let renewed = renew(&next.fence(), Some(&taken), &active, 10_000, 5_000)
        .expect("asked")
        .expect("its own");
    assert_eq!(
        (renewed.fence, renewed.acquired_at_ms, renewed.expires_at_ms),
        (next.fence, 2_000, 15_000)
    );
    assert!(renew(&next.fence(), Some(&taken), &active, 0, 5_000).is_err());
}

#[test]
fn a_store_is_activated_at_a_new_epoch_and_deactivated_at_the_one_it_has() {
    let inactive = standing(StoreActivation::Inactive, 0);
    let (active, event) =
        activation_change(&inactive, StoreActivation::Active, 5).expect("activated");
    assert_eq!(
        (active.activation, active.epoch.0),
        (StoreActivation::Active, 1)
    );
    assert_eq!(
        (event.from, event.to, event.epoch.0, event.at_ms),
        (StoreActivation::Inactive, StoreActivation::Active, 1, 5)
    );

    let again = activation_change(&active, StoreActivation::Active, 6).expect_err("once");
    assert!(again.to_string().contains("already active, at epoch 1"));
    let (handed_over, event) =
        activation_change(&active, StoreActivation::Deactivated, 7).expect("deactivated");
    assert_eq!(event.epoch.0, 1, "a deactivation keeps the epoch");
    let (back, _) =
        activation_change(&handed_over, StoreActivation::Active, 8).expect("reactivated");
    assert_eq!(back.epoch.0, 2);

    for idle in [&inactive, &handed_over] {
        let refused = activation_change(idle, StoreActivation::Deactivated, 9);
        assert!(
            refused
                .expect_err("not active")
                .to_string()
                .contains("only an active store")
        );
    }
    let unbound = StoreStanding {
        environment: None,
        ..inactive
    };
    let refused = activation_change(&unbound, StoreActivation::Active, 9);
    assert!(
        refused
            .expect_err("unbound")
            .to_string()
            .contains("bind it")
    );
}
