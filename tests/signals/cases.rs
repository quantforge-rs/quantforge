//! The shared signal cases: the bodies, what each one is, the product
//! lines with their profiles and what their venues list, and what the
//! Python model recorded of each signal. The same reading gives every test
//! here its payloads, its policies and its instrument lists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use quantforge::model::futures::{
    DefaultsEdit, Edit, ExternalSignal, FuturesInstrument, InstrumentSpec, NewAsset, PolicyChange,
    ProfileId, ProfileIdentity, ProfilePolicy, RiskPercent, StopFallback, StopPercent,
};
use quantforge::model::{Leverage, Money, Symbol, TimestampMs};
use quantforge::server::read_signal;
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fixture {
    pub(crate) format: u32,
    pub(crate) generated_by: String,
    pub(crate) signal_version: u32,
    pub(crate) max_signal_bytes: usize,
    pub(crate) max_depth: usize,
    pub(crate) ticker_max: usize,
    /// The code points a ticker may not hold beyond ASCII, as ranges.
    pub(crate) ticker_refused: Vec<(u32, u32)>,
    pub(crate) age_limit_s: AgeLimits,
    pub(crate) future_allowance_ms: i64,
    pub(crate) fields: Vec<String>,
    pub(crate) entry_only: Vec<String>,
    pub(crate) required: BTreeMap<String, Vec<String>>,
    pub(crate) id_fields: Vec<String>,
    pub(crate) not_in_canonical: Vec<String>,
    pub(crate) problem_codes: Vec<String>,
    pub(crate) receipt_states: Vec<String>,
    pub(crate) receipt_fields: Vec<String>,
    pub(crate) spellings: Vec<String>,
    /// The parts of a signal a refusal can be over, in the order the rules
    /// are looked for.
    pub(crate) resolution_order: Vec<String>,
    pub(crate) schema_sha256: String,
    /// What each placeholder of the templates is rendered with.
    pub(crate) rendered_with: BTreeMap<String, String>,
    pub(crate) lines: Vec<Line>,
    pub(crate) payloads: Vec<Payload>,
    pub(crate) templates: Vec<Template>,
    pub(crate) resolutions: Vec<Resolution>,
    pub(crate) exits: Vec<ExitCase>,
    pub(crate) deliveries: Vec<DeliveryCase>,
    pub(crate) held: Vec<HeldCase>,
    pub(crate) receipts: Vec<ReceiptCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgeLimits {
    pub(crate) default: u32,
    pub(crate) min: u32,
    pub(crate) max: u32,
}

/// One product line: a profile, the changes that make its policy, and what
/// its venue lists.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Line {
    pub(crate) name: String,
    pub(crate) identity: ProfileIdentity,
    pub(crate) changes: Vec<Change>,
    pub(crate) instruments: Vec<InstrumentSpec>,
}

/// One change to a profile's policy, each the next revision.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Change {
    Defaults(DefaultsChange),
    Capital(Money),
    Asset(AssetChange),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DefaultsChange {
    pub(crate) enabled: bool,
    pub(crate) risk_percent: Option<RiskPercent>,
    pub(crate) stop_percent: Option<StopPercent>,
    pub(crate) leverage: Option<Leverage>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssetChange {
    pub(crate) symbol: Symbol,
    pub(crate) enabled: bool,
    pub(crate) risk_percent: Option<RiskPercent>,
    pub(crate) stop: StopFallback,
    pub(crate) leverage: Option<Leverage>,
}

/// One body, and what the Python model found it to be.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Payload {
    pub(crate) id: String,
    pub(crate) note: String,
    /// The body as text, where it is text.
    pub(crate) body: Option<String>,
    /// The body in hex, where its bytes are not UTF-8.
    pub(crate) body_hex: Option<String>,
    pub(crate) signal: Option<SignalAnswer>,
    pub(crate) problems: Option<Vec<ProblemAnswer>>,
    /// What the JSON schema alone says of the body; `None` where the body
    /// is no JSON for the schema to be asked about.
    pub(crate) schema_accepts: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalAnswer {
    pub(crate) fields: Map<String, Value>,
    pub(crate) signal_id: String,
    pub(crate) canonical: String,
    pub(crate) hash: String,
    pub(crate) bar_time_ms: TimestampMs,
    pub(crate) fire_time_ms: TimestampMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProblemAnswer {
    pub(crate) code: String,
    pub(crate) field: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Template {
    pub(crate) name: String,
    pub(crate) note: String,
    pub(crate) template: String,
    pub(crate) placeholders: Vec<String>,
    pub(crate) rendered: String,
    pub(crate) signal_id: String,
    pub(crate) canonical: String,
}

/// One signal on one line, at one time, and what it resolves to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resolution {
    pub(crate) id: String,
    pub(crate) note: String,
    pub(crate) line: String,
    pub(crate) payload: String,
    pub(crate) now_ms: TimestampMs,
    pub(crate) age_limit_s: u32,
    /// Every part of the signal that breaks its rule, each asked on its
    /// own; the signal is refused over the first.
    pub(crate) breaks: Vec<String>,
    pub(crate) outcome: Outcome,
}

/// What a signal resolves to: an entry or an exit, or a refusal and the
/// part of the signal it is over.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Outcome {
    /// The entry or the exit, and an entry's policy in its stored form.
    pub(crate) resolved: Option<Value>,
    pub(crate) refused: Option<String>,
    pub(crate) over: Option<String>,
}

/// A payload on a line, fired at a time of its own.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fired {
    pub(crate) line: String,
    pub(crate) payload: String,
    pub(crate) fire_time: String,
    pub(crate) fire_time_ms: TimestampMs,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExitCase {
    pub(crate) id: String,
    pub(crate) note: String,
    /// The entry whose position is open, or `None` where nothing is.
    pub(crate) entry: Option<Fired>,
    pub(crate) exit: Fired,
    pub(crate) closes: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeliveryCase {
    pub(crate) id: String,
    pub(crate) note: String,
    pub(crate) steps: Vec<DeliveryStep>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeliveryStep {
    pub(crate) profile: String,
    pub(crate) payload: String,
    pub(crate) outcome: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeldCase {
    pub(crate) held: Option<String>,
    pub(crate) entry: String,
    pub(crate) refused: Option<String>,
    pub(crate) note: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptCase {
    pub(crate) id: String,
    pub(crate) note: String,
    pub(crate) body: Value,
    pub(crate) valid: bool,
}

pub(crate) fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("signal_cases.json")
}

pub(crate) fn fixture() -> Fixture {
    let text = std::fs::read_to_string(fixture_path()).expect("read the signal cases");
    serde_json::from_str(&text).expect("the signal cases")
}

/// A file of the repository, as text.
pub(crate) fn read(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

impl Payload {
    /// The body's bytes, exactly as a delivery would carry them.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        match (&self.body, &self.body_hex) {
            (Some(text), None) => text.clone().into_bytes(),
            (None, Some(hex)) => hex::decode(hex).expect("a body in hex"),
            _ => panic!("{}: a body is text or hex, one of them", self.id),
        }
    }
}

impl Fixture {
    pub(crate) fn payload(&self, id: &str) -> &Payload {
        self.payloads
            .iter()
            .find(|payload| payload.id == id)
            .unwrap_or_else(|| panic!("the fixture has no payload {id}"))
    }

    /// The model's signal for a payload the fixture says is one.
    pub(crate) fn signal(&self, id: &str) -> ExternalSignal {
        read_signal(&self.payload(id).bytes()).unwrap_or_else(|refused| panic!("{id}: {refused:?}"))
    }

    pub(crate) fn line(&self, name: &str) -> &Line {
        self.lines
            .iter()
            .find(|line| line.name == name)
            .unwrap_or_else(|| panic!("the fixture has no line {name}"))
    }

    /// The line whose profile has this ID.
    pub(crate) fn line_of(&self, profile: &str) -> &Line {
        self.lines
            .iter()
            .find(|line| line.identity.id().as_str() == profile)
            .unwrap_or_else(|| panic!("the fixture has no profile {profile}"))
    }

    /// What a connector hands the resolver for `line`: the line's own
    /// instruments, and those of every line of another product, which a
    /// resolver must never match.
    pub(crate) fn listed_with_others(&self, line: &Line) -> Vec<FuturesInstrument> {
        let own = line.identity.category();
        let venue = line.identity.account_ref().venue();
        let mut listed = line.listed();
        for other in &self.lines {
            let identity = &other.identity;
            if identity.category() != own || identity.account_ref().venue() != venue {
                listed.extend(other.listed());
            }
        }
        listed
    }
}

/// A setting a change gives, or leaves as it is.
fn set<T>(value: Option<T>) -> Edit<T> {
    value.map_or(Edit::Keep, Edit::Set)
}

impl Line {
    pub(crate) fn profile(&self) -> &ProfileId {
        self.identity.id()
    }

    /// What the line's venue lists.
    pub(crate) fn listed(&self) -> Vec<FuturesInstrument> {
        self.instruments
            .iter()
            .cloned()
            .map(|spec| FuturesInstrument::try_from(spec).expect("an instrument"))
            .collect()
    }

    /// The operator's changes, in the order they are made.
    pub(crate) fn changes(&self) -> Vec<PolicyChange> {
        self.changes
            .iter()
            .map(|change| match change {
                Change::Defaults(defaults) => PolicyChange::Defaults(DefaultsEdit {
                    enabled: Some(defaults.enabled),
                    risk_percent: set(defaults.risk_percent),
                    stop_percent: set(defaults.stop_percent),
                    leverage: set(defaults.leverage),
                }),
                Change::Capital(amount) => PolicyChange::SetCapital(amount.clone()),
                Change::Asset(asset) => {
                    let listed = self
                        .instruments
                        .iter()
                        .find(|spec| spec.id.symbol == asset.symbol)
                        .unwrap_or_else(|| panic!("{} lists no {}", self.name, asset.symbol));
                    PolicyChange::AddAsset(NewAsset {
                        instrument: listed.id.clone(),
                        enabled: asset.enabled,
                        risk_percent: asset.risk_percent,
                        stop: asset.stop,
                        leverage: asset.leverage,
                    })
                }
            })
            .collect()
    }

    /// The profile's policy once every change is made, each a revision.
    pub(crate) fn policy(&self) -> ProfilePolicy {
        let mut policy = ProfilePolicy::new(self.identity.clone());
        for change in self.changes() {
            policy = policy
                .apply(&change)
                .unwrap_or_else(|error| panic!("{}: {error}", self.name))
                .unwrap_or_else(|| panic!("{}: {change:?} changes nothing", self.name));
        }
        policy
    }
}
