//! The policy service: a store's automation profiles and their policies,
//! as an operator's commands and an entry's admission use them.
//!
//! - **Changes** are made revision by revision. A caller that read revision
//!   N names it, and is told when the profile has moved on meanwhile; one
//!   that names none has its change applied to whatever is newest.
//! - **The venue's adapter decides what exists.** An asset is added as the
//!   venue lists it, settlement asset included, and a change that would
//!   enable something the venue does not take is refused before it is
//!   stored.
//! - **Before a profile may execute**, [`PolicyService::check`] holds its
//!   whole policy to the venue's list.
//! - **An entry** gets the policy it is admitted under from
//!   [`PolicyService::entry`], taken from the newest revision and never
//!   from the signal. An exit takes no policy at all.

use std::fmt;

use thiserror::Error;

use crate::model::futures::OwnerTag;
use crate::model::futures::{
    FuturesInstrument, NewAsset, PolicyChange, PolicyProblem, PolicyProblemKind, PolicyRefusal,
    PolicyRevision, PolicySnapshot, PolicyStamp, ProfileId, ProfileIdentity, ProfilePolicy,
    RiskPercent, StopFallback,
};
use crate::model::{AccountRef, Category, Leverage, ModelError, Symbol};
use crate::ports::futures::InstrumentConnector;
use crate::ports::{Clock, ExchangeError, PolicyStore, StorageError};

/// How many times a change that names no revision is applied again when
/// another change got in first, before it gives up.
const ATTEMPTS: usize = 8;

/// Why the policy service did not do what it was asked.
#[derive(Debug, Error)]
pub enum PolicyError {
    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Model(#[from] ModelError),

    #[error(transparent)]
    Exchange(#[from] ExchangeError),

    #[error("the store holds no profile {0}; create it first")]
    UnknownProfile(ProfileId),

    #[error("profile {profile} holds no revision {revision} of its policy")]
    UnknownRevision {
        profile: ProfileId,
        revision: PolicyRevision,
    },

    #[error(
        "profile {profile} is at revision {found}, not {expected}: it was changed in the \
         meantime; read it again and repeat the change"
    )]
    Moved {
        profile: ProfileId,
        expected: PolicyRevision,
        found: PolicyRevision,
    },

    #[error("profile {0} kept changing while this change was made; try again")]
    Busy(ProfileId),

    #[error("{connector} is not the connector of profile {profile}, which trades {profile_line}")]
    WrongConnector {
        profile: ProfileId,
        profile_line: String,
        connector: String,
    },

    #[error("the venue lists no {line} contract {symbol}")]
    NotListed { line: String, symbol: Symbol },

    /// The venue does not take what the change would enable.
    #[error("the venue does not take the change: {}", listed(.0))]
    Rejected(Vec<PolicyProblem>),

    /// The policy refuses the entry.
    #[error("{0}")]
    Refused(PolicyRefusal),
}

fn listed(problems: &[PolicyProblem]) -> String {
    problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A profile's policy after a change, and whether the change made a
/// revision: one that asks for what the policy already is makes none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyOutcome {
    pub policy: ProfilePolicy,
    pub changed: bool,
}

/// The settings a new asset is given. A setting left out is the defaults'.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetSettings {
    pub enabled: bool,
    pub risk_percent: Option<RiskPercent>,
    pub stop: StopFallback,
    pub leverage: Option<Leverage>,
}

/// A policy held against its venue's instrument list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyCheck {
    /// The revision that was checked.
    pub revision: PolicyRevision,
    /// Every reason the profile cannot execute as that revision stands.
    pub problems: Vec<PolicyProblem>,
}

impl PolicyCheck {
    /// Whether the profile may execute: every enabled asset is a contract
    /// the venue lists, inside the universe, with a leverage the venue
    /// takes and capital to size from.
    pub fn is_ready(&self) -> bool {
        self.problems.is_empty()
    }
}

/// The policy service over one store.
pub struct PolicyService<'a> {
    store: &'a dyn PolicyStore,
    clock: &'a dyn Clock,
}

impl fmt::Debug for PolicyService<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PolicyService").finish_non_exhaustive()
    }
}

impl<'a> PolicyService<'a> {
    pub fn new(store: &'a dyn PolicyStore, clock: &'a dyn Clock) -> Self {
        Self { store, clock }
    }

    /// Creates a profile for `account`'s `category` contracts, with a fresh
    /// owner tag and an empty policy: no settings, no capital, nothing
    /// enabled. The store refuses an ID it already holds, and an account of
    /// another environment than its own.
    pub fn create_profile(
        &self,
        id: ProfileId,
        account: AccountRef,
        category: Category,
    ) -> Result<ProfilePolicy, PolicyError> {
        let identity = ProfileIdentity::new(id, account, category, OwnerTag::generate())?;
        let policy = ProfilePolicy::new(identity);
        self.store.create_profile(&policy, self.clock.now_ms())?;
        Ok(policy)
    }

    /// Every profile of the store, in the byte order of their IDs.
    pub fn profiles(&self) -> Result<Vec<ProfileIdentity>, PolicyError> {
        Ok(self.store.profiles()?)
    }

    /// The newest revision of the profile's policy.
    pub fn policy(&self, profile: &ProfileId) -> Result<ProfilePolicy, PolicyError> {
        self.store
            .profile_policy(profile)?
            .ok_or_else(|| PolicyError::UnknownProfile(profile.clone()))
    }

    /// The profile's policy as it stood at `revision`: the one an entry
    /// admitted under that revision was sized from.
    pub fn policy_at(
        &self,
        profile: &ProfileId,
        revision: PolicyRevision,
    ) -> Result<ProfilePolicy, PolicyError> {
        match self.store.profile_policy_at(profile, revision)? {
            Some(policy) => Ok(policy),
            None => {
                // Tells a profile the store does not hold from a revision
                // the profile does not have.
                self.policy(profile)?;
                Err(PolicyError::UnknownRevision {
                    profile: profile.clone(),
                    revision,
                })
            }
        }
    }

    /// When each revision of the profile's policy was made, oldest first.
    pub fn history(&self, profile: &ProfileId) -> Result<Vec<PolicyStamp>, PolicyError> {
        let stamps = self.store.policy_revisions(profile)?;
        if stamps.is_empty() {
            return Err(PolicyError::UnknownProfile(profile.clone()));
        }
        Ok(stamps)
    }

    /// Makes `change` to the profile's policy, as its next revision. With
    /// `expected`, the change is made only while the profile is at that
    /// revision, the one its caller read. Without, it is made to whatever
    /// is newest, again if another change gets in first.
    ///
    /// Only the policy's own rules are checked here; a change that brings
    /// in an asset or a leverage goes through
    /// [`PolicyService::change_checked`], which asks the venue.
    pub fn change(
        &self,
        profile: &ProfileId,
        expected: Option<PolicyRevision>,
        change: &PolicyChange,
    ) -> Result<PolicyOutcome, PolicyError> {
        self.change_with(profile, expected, change, |_, _| Ok(()))
    }

    /// [`PolicyService::change`], with the venue's say: the change is
    /// refused, and nothing stored, when it would leave an enabled asset
    /// that the venue does not list on the profile's product line, lists
    /// under another settlement asset, keeps outside the universe, or
    /// takes no such leverage for. Capital is not the venue's to refuse:
    /// [`PolicyService::check`] reports an asset left without any.
    pub async fn change_checked(
        &self,
        profile: &ProfileId,
        expected: Option<PolicyRevision>,
        change: &PolicyChange,
        venue: &dyn InstrumentConnector,
    ) -> Result<PolicyOutcome, PolicyError> {
        let instruments = self.listed(&self.policy(profile)?, venue).await?;
        self.change_against(profile, expected, change, &instruments)
    }

    /// [`PolicyService::change`], refused when it brings a problem the
    /// venue's `instruments` show and the policy did not have before it.
    fn change_against(
        &self,
        profile: &ProfileId,
        expected: Option<PolicyRevision>,
        change: &PolicyChange,
        instruments: &[FuturesInstrument],
    ) -> Result<PolicyOutcome, PolicyError> {
        self.change_with(profile, expected, change, |before, after| {
            let known = before.check(instruments);
            let brought: Vec<PolicyProblem> = after
                .check(instruments)
                .into_iter()
                .filter(|problem| problem.kind != PolicyProblemKind::NoCapital)
                .filter(|problem| !known.contains(problem))
                .collect();
            if brought.is_empty() {
                Ok(())
            } else {
                Err(PolicyError::Rejected(brought))
            }
        })
    }

    /// Gives `symbol` settings of its own, as the venue lists it now: the
    /// settlement asset is the venue's, never the caller's. Refuses a
    /// symbol the venue does not list on the profile's product line, and,
    /// for an asset added enabled, whatever
    /// [`PolicyService::change_checked`] refuses.
    pub async fn add_asset(
        &self,
        profile: &ProfileId,
        expected: Option<PolicyRevision>,
        symbol: &Symbol,
        settings: AssetSettings,
        venue: &dyn InstrumentConnector,
    ) -> Result<PolicyOutcome, PolicyError> {
        let policy = self.policy(profile)?;
        let instruments = self.listed(&policy, venue).await?;
        let line = policy.exchange();
        let listed = instruments
            .iter()
            .find(|listed| listed.id().exchange == line && &listed.id().symbol == symbol)
            .ok_or_else(|| PolicyError::NotListed {
                line: format!("{} {}", line.venue(), line.category()),
                symbol: symbol.clone(),
            })?;
        let change = PolicyChange::AddAsset(NewAsset {
            instrument: listed.id().clone(),
            enabled: settings.enabled,
            risk_percent: settings.risk_percent,
            stop: settings.stop,
            leverage: settings.leverage,
        });
        // Held to the list the asset was just read from.
        self.change_against(profile, expected, &change, &instruments)
    }

    /// Holds the profile's newest policy to the venue's instrument list:
    /// what must be clean before the profile may execute.
    pub async fn check(
        &self,
        profile: &ProfileId,
        venue: &dyn InstrumentConnector,
    ) -> Result<PolicyCheck, PolicyError> {
        let policy = self.policy(profile)?;
        let instruments = self.listed(&policy, venue).await?;
        Ok(PolicyCheck {
            revision: policy.revision(),
            problems: policy.check(&instruments),
        })
    }

    /// The policy an entry on `instrument` is admitted under, from the
    /// profile's newest revision, or the refusal the entry is recorded
    /// with. `instrument` is the venue's listing, read now. Nothing of a
    /// signal reaches this: its only say is its own stop, which
    /// [`PolicySnapshot::stop_for`] answers for next, with `no_stop` for an
    /// entry that has neither stop; only then is the entry admitted.
    pub fn entry(
        &self,
        profile: &ProfileId,
        instrument: &FuturesInstrument,
    ) -> Result<PolicySnapshot, PolicyError> {
        self.policy(profile)?
            .entry_snapshot(instrument)
            .map_err(PolicyError::Refused)
    }

    /// The venue's instruments, once `venue` is known to be the connector
    /// of the policy's own product line and environment.
    async fn listed(
        &self,
        policy: &ProfilePolicy,
        venue: &dyn InstrumentConnector,
    ) -> Result<Vec<FuturesInstrument>, PolicyError> {
        let identity = policy.identity();
        let line = policy.exchange();
        let environment = identity.account_ref().environment();
        if venue.exchange_id() != line || venue.environment() != environment {
            let connector = venue.exchange_id();
            return Err(PolicyError::WrongConnector {
                profile: identity.id().clone(),
                profile_line: format!("{} {} on {environment}", line.venue(), line.category()),
                connector: format!(
                    "a {} {} connector for {}",
                    connector.venue(),
                    connector.category(),
                    venue.environment()
                ),
            });
        }
        Ok(venue.instruments().await?)
    }

    /// Applies `change` and stores the revision it makes, once `accept`
    /// takes the policy before and after.
    fn change_with(
        &self,
        profile: &ProfileId,
        expected: Option<PolicyRevision>,
        change: &PolicyChange,
        accept: impl Fn(&ProfilePolicy, &ProfilePolicy) -> Result<(), PolicyError>,
    ) -> Result<PolicyOutcome, PolicyError> {
        for _ in 0..ATTEMPTS {
            let current = self.policy(profile)?;
            let moved = |found: PolicyRevision| {
                expected
                    .filter(|expected| *expected != found)
                    .map(|expected| PolicyError::Moved {
                        profile: profile.clone(),
                        expected,
                        found,
                    })
            };
            if let Some(moved) = moved(current.revision()) {
                return Err(moved);
            }
            let Some(next) = current.apply(change)? else {
                return Ok(PolicyOutcome {
                    policy: current,
                    changed: false,
                });
            };
            accept(&current, &next)?;
            if self.store.save_policy(&next, self.clock.now_ms())? {
                return Ok(PolicyOutcome {
                    policy: next,
                    changed: true,
                });
            }
            // Another change was stored first. A caller that named its
            // revision is told; one that named none goes again.
            if let Some(moved) = moved(self.policy(profile)?.revision()) {
                return Err(moved);
            }
        }
        Err(PolicyError::Busy(profile.clone()))
    }
}
