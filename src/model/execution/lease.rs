//! The lease on an execution position (ADR 0002 §2): who may act on it,
//! and the fence token that tells a holder's writes from a former
//! holder's.

use crate::model::futures::{ActivationEpoch, PositionKey, ProfileId};
use crate::model::identity::{plain_text, text_id};
use crate::model::{ModelError, TimestampMs};
use std::fmt;

text_id!(
    /// Who holds a lease: one running executor, named so that no two
    /// share a name, such as a host, a process and a random part.
    LeaseHolder,
    "lease holder",
    "1 to 64 characters without spaces",
    |text| plain_text(text, 64)
);

/// A lease's fence token. Every grant of a position's lease takes the
/// next one, so a write that names an earlier token is a former holder's,
/// whatever its clock says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FenceToken(u64);

impl FenceToken {
    /// Refuses 0, which no store issues.
    pub fn new(value: u64) -> Result<Self, ModelError> {
        if value == 0 {
            return Err(ModelError::InvalidIdentifier {
                what: "fence token",
                value: value.to_string(),
                rule: "positive",
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FenceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A lease as its store holds it: one position, one holder, until it
/// expires, is released or the store that granted it stops being active
/// at the epoch it was granted under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub key: PositionKey,
    /// The profile the holder acts for.
    pub profile: ProfileId,
    pub holder: LeaseHolder,
    pub fence: FenceToken,
    /// The store's activation epoch when the lease was granted.
    pub epoch: ActivationEpoch,
    pub acquired_at_ms: TimestampMs,
    pub expires_at_ms: TimestampMs,
}

impl Lease {
    /// What a unit carries to be written under this lease.
    pub fn fence(&self) -> Fence {
        Fence {
            key: self.key.clone(),
            holder: self.holder.clone(),
            token: self.fence,
        }
    }

    /// Whether another holder may take the position at `now_ms`.
    pub const fn is_expired(&self, now_ms: TimestampMs) -> bool {
        self.expires_at_ms <= now_ms
    }
}

/// The proof a unit is written under a lease: the position, the holder
/// and the token it was granted. A store applies the unit only while its
/// lease row still shows exactly these, on a store active at the lease's
/// epoch. Time is not part of it: a lease that ran out and was taken by
/// no one else still fences, and one that was taken does not, however the
/// clocks stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fence {
    pub key: PositionKey,
    pub holder: LeaseHolder,
    pub token: FenceToken,
}

/// A holder asking for a position's lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseRequest {
    pub key: PositionKey,
    pub profile: ProfileId,
    pub holder: LeaseHolder,
    /// How long the lease lasts unless renewed, in milliseconds.
    pub ttl_ms: i64,
    pub now_ms: TimestampMs,
}

/// What a store answers a request with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseAnswer {
    /// The lease is the asker's.
    Granted(Lease),
    /// Another holder has it, until it expires or is released.
    Held(Lease),
}

impl LeaseRequest {
    /// The lease the request is answered with on a store active at
    /// `epoch`, where the position's lease stands as `current` and the
    /// last token issued for it was `last_fence`, 0 when none ever was.
    ///
    /// A lease held by another, not yet expired and granted under this
    /// epoch, is left alone. The asker's own such lease is extended and
    /// keeps its token. Anything else is a new grant with the next token:
    /// no lease, an expired one, or one granted under an earlier epoch,
    /// which the store's reactivation voided.
    pub fn decide(
        &self,
        current: Option<&Lease>,
        last_fence: u64,
        epoch: ActivationEpoch,
    ) -> Result<LeaseAnswer, ModelError> {
        let expires_at_ms = self.expiry()?;
        let live = current.filter(|held| held.epoch == epoch && !held.is_expired(self.now_ms));
        if let Some(held) = live {
            if held.holder != self.holder {
                return Ok(LeaseAnswer::Held(held.clone()));
            }
            return Ok(LeaseAnswer::Granted(Lease {
                profile: self.profile.clone(),
                expires_at_ms,
                ..held.clone()
            }));
        }
        let next = last_fence
            .checked_add(1)
            .ok_or_else(|| ModelError::InvalidIdentifier {
                what: "fence token",
                value: last_fence.to_string(),
                rule: "followed by another",
            })?;
        Ok(LeaseAnswer::Granted(Lease {
            key: self.key.clone(),
            profile: self.profile.clone(),
            holder: self.holder.clone(),
            fence: FenceToken::new(next)?,
            epoch,
            acquired_at_ms: self.now_ms,
            expires_at_ms,
        }))
    }

    fn expiry(&self) -> Result<TimestampMs, ModelError> {
        let invalid = || ModelError::InvalidAmount {
            what: "lease duration",
            value: format!("{} ms", self.ttl_ms),
            rule: "above zero, and ending at a time a clock can hold",
        };
        if self.ttl_ms <= 0 {
            return Err(invalid());
        }
        self.now_ms.checked_add(self.ttl_ms).ok_or_else(invalid)
    }
}

impl Fence {
    /// Whether the fence is `lease`'s own: the same position, holder and
    /// token.
    pub fn is_of(&self, lease: &Lease) -> bool {
        self.key == lease.key && self.holder == lease.holder && self.token == lease.fence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountRef, Asset, Environment, ExchangeId, InstrumentId, Symbol, Venue, VenueAccountId,
    };

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
    fn each_grant_takes_the_next_token_and_a_live_lease_is_left_to_its_holder() {
        let epoch = ActivationEpoch(1);
        let first = granted(request("a", 1_000).decide(None, 0, epoch).expect("decided"));
        assert_eq!((first.fence.get(), first.expires_at_ms), (1, 31_000));

        // Another holder waits while the lease lives.
        let waiting = request("b", 30_999).decide(Some(&first), 1, epoch);
        assert_eq!(waiting.expect("decided"), LeaseAnswer::Held(first.clone()));

        // At its expiry it is anyone's, under the next token.
        let taken = granted(
            request("b", 31_000)
                .decide(Some(&first), 1, epoch)
                .expect("decided"),
        );
        assert_eq!((taken.fence.get(), taken.holder.as_str()), (2, "b"));
        assert!(!first.fence().is_of(&taken), "the former holder is fenced");
        assert!(taken.fence().is_of(&taken));
    }

    #[test]
    fn a_holder_asking_again_keeps_its_token_and_a_lease_of_another_epoch_is_void() {
        let epoch = ActivationEpoch(1);
        let first = granted(request("a", 1_000).decide(None, 0, epoch).expect("decided"));
        let again = granted(
            request("a", 20_000)
                .decide(Some(&first), 1, epoch)
                .expect("decided"),
        );
        assert_eq!((again.fence, again.expires_at_ms), (first.fence, 50_000));
        assert_eq!(again.acquired_at_ms, first.acquired_at_ms);

        // The store was reactivated: what the earlier epoch granted is gone,
        // for its own holder too.
        let after = granted(
            request("a", 20_000)
                .decide(Some(&first), 1, ActivationEpoch(2))
                .expect("decided"),
        );
        assert_eq!((after.fence.get(), after.epoch), (2, ActivationEpoch(2)));
    }

    #[test]
    fn a_lease_lasts_some_time_and_ends_at_a_time_a_clock_holds() {
        let mut asked = request("a", 1_000);
        asked.ttl_ms = 0;
        assert!(asked.decide(None, 0, ActivationEpoch(1)).is_err());
        asked.ttl_ms = i64::MAX;
        assert!(asked.decide(None, 0, ActivationEpoch(1)).is_err());
        assert!(FenceToken::new(0).is_err());
        assert!(LeaseHolder::new("host a").is_err(), "a space");
    }
}
