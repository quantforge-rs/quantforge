//! The shared sizing cases: what each one hands sizing, in the shared
//! model's stored form, and the answer the Python model recorded. The same
//! reading turns a case into a sizing request for every test here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use quantforge::model::futures::{
    AccountSnapshot, FeeRates, FuturesInstrument, InstrumentSpec, LeverageBracket, NewAsset,
    Observed, PolicyChange, PolicySnapshot, PriceSnapshot, ProfileIdentity, ProfilePolicy,
    RiskPercent, SizedEntry, Sizing, SizingFault, SizingFigures, SizingRequest, StopFallback,
    StopPercent, SubmissionCheck, UsdIndex,
};
use quantforge::model::{Asset, Leverage, Money, PositionSide, Price, Quantity, TimestampMs};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fixture {
    pub(crate) format: u32,
    pub(crate) generated_by: String,
    #[allow(dead_code, reason = "how the Python model computes, not a rule")]
    pub(crate) decimal_precision: u32,
    pub(crate) rounding: BTreeMap<String, String>,
    pub(crate) drift_limit: String,
    pub(crate) freshness_ms: FreshnessMs,
    pub(crate) now_ms: TimestampMs,
    /// The venues' values as recorded; `instruments` and `tiers` are what
    /// an adapter makes of them, and what sizing reads.
    #[allow(
        dead_code,
        reason = "the venues' raw values, kept beside their normalized form"
    )]
    pub(crate) rules: Value,
    #[allow(
        dead_code,
        reason = "the venues' raw values, kept beside their normalized form"
    )]
    pub(crate) brackets: Value,
    pub(crate) instruments: BTreeMap<String, InstrumentSpec>,
    pub(crate) tiers: BTreeMap<String, Vec<LeverageBracket>>,
    pub(crate) sizing: Vec<Case>,
    pub(crate) fills: Vec<FillCase>,
    #[allow(dead_code, reason = "the fraction helper's unit tests read them")]
    pub(crate) arithmetic: Vec<Value>,
    #[allow(
        dead_code,
        reason = "accounting's, kept with the cases ADR 0002 records"
    )]
    pub(crate) cycles: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreshnessMs {
    pub(crate) prices: i64,
    pub(crate) account: i64,
    pub(crate) rules: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) note: String,
    /// The parity matrix's cell, such as `futures.sizing/bybit/linear/short`.
    pub(crate) cell: String,
    pub(crate) inputs: Inputs,
    /// What was worked out by hand before the model ran.
    #[allow(dead_code, reason = "the Python model checks itself against it")]
    pub(crate) expect: Value,
    pub(crate) result: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Inputs {
    pub(crate) profile: ProfileIdentity,
    pub(crate) policy: PolicyInputs,
    /// The snapshot the entry is admitted under, or `null` where the
    /// profile's policy refuses it.
    pub(crate) snapshot: Option<Value>,
    pub(crate) side: PositionSide,
    pub(crate) expected_price: Price,
    pub(crate) signal_stop: Option<Price>,
    /// The instrument whose rules sizing is given, by its key.
    pub(crate) instrument: String,
    pub(crate) rules_at_ms: TimestampMs,
    pub(crate) brackets: Tiers,
    pub(crate) brackets_at_ms: TimestampMs,
    pub(crate) fees: FeeRates,
    pub(crate) fees_at_ms: TimestampMs,
    pub(crate) account: AccountSnapshot,
    pub(crate) reserved: Money,
    pub(crate) usd_index: Option<IndexInput>,
    pub(crate) prices: Option<PriceSnapshot>,
    pub(crate) now_ms: TimestampMs,
}

/// What the operator set: the capital, and the asset's own settings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyInputs {
    pub(crate) capital: Money,
    pub(crate) risk_percent: String,
    pub(crate) stop_percent: Option<String>,
    pub(crate) leverage: String,
}

/// An instrument's brackets by its key, or a list a case gives itself.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Tiers {
    Of(String),
    Given(Vec<LeverageBracket>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexInput {
    pub(crate) asset: Asset,
    pub(crate) price: Price,
    pub(crate) source: String,
    pub(crate) observed_at_ms: TimestampMs,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FillCase {
    pub(crate) id: String,
    #[allow(dead_code, reason = "says what the case shows")]
    pub(crate) note: String,
    /// The sizing case whose entry filled.
    pub(crate) entry: String,
    pub(crate) fills: Vec<FillInput>,
    #[allow(dead_code, reason = "the Python model checks itself against it")]
    pub(crate) expect: Value,
    pub(crate) result: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FillInput {
    pub(crate) price: Price,
    pub(crate) quantity: Quantity,
}

pub(crate) fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sizing_cases.json")
}

pub(crate) fn fixture() -> Fixture {
    let text = std::fs::read_to_string(fixture_path()).expect("read the sizing cases");
    serde_json::from_str(&text).expect("the sizing cases")
}

pub(crate) fn decimal(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

impl Fixture {
    pub(crate) fn case(&self, id: &str) -> &Case {
        self.sizing
            .iter()
            .find(|case| case.id == id)
            .unwrap_or_else(|| panic!("the fixture has no case {id}"))
    }

    /// The instrument under `key`, as a venue lists it, with its rules
    /// read at `observed_at_ms`.
    pub(crate) fn listed(&self, key: &str, observed_at_ms: TimestampMs) -> FuturesInstrument {
        let mut spec = self
            .instruments
            .get(key)
            .unwrap_or_else(|| panic!("the fixture has no instrument {key}"))
            .clone();
        spec.observed_at_ms = observed_at_ms;
        FuturesInstrument::try_from(spec).expect("an instrument")
    }
}

impl Case {
    /// The instrument the entry is admitted on: the one its snapshot
    /// names, which is the one sizing is given unless the case hands
    /// sizing another's rules on purpose.
    pub(crate) fn admitted_on(&self, fixture: &Fixture) -> FuturesInstrument {
        let named = self
            .inputs
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot["instrument"].clone());
        let key = fixture
            .instruments
            .iter()
            .find(|(_, spec)| {
                named
                    .as_ref()
                    .is_some_and(|id| &serde_json::to_value(&spec.id).expect("an id") == id)
            })
            .map_or(self.inputs.instrument.as_str(), |(key, _)| key.as_str());
        fixture.listed(key, self.inputs.rules_at_ms)
    }

    /// The two changes that give a new profile the case's policy: its
    /// capital, then the asset with settings of its own.
    pub(crate) fn changes(&self, listed: &FuturesInstrument) -> [PolicyChange; 2] {
        let policy = &self.inputs.policy;
        let stop = policy
            .stop_percent
            .as_deref()
            .map_or(StopFallback::Off, |percent| {
                StopFallback::Percent(
                    StopPercent::new(decimal(percent)).expect("a stop percentage"),
                )
            });
        [
            PolicyChange::SetCapital(policy.capital.clone()),
            PolicyChange::AddAsset(NewAsset {
                instrument: listed.id().clone(),
                enabled: true,
                risk_percent: Some(
                    RiskPercent::new(decimal(&policy.risk_percent)).expect("a risk percentage"),
                ),
                stop,
                leverage: Some(Leverage::new(decimal(&policy.leverage)).expect("a leverage")),
            }),
        ]
    }

    /// The policy of a profile made for the case, on the pure model.
    pub(crate) fn policy(&self, listed: &FuturesInstrument) -> ProfilePolicy {
        self.changes(listed).iter().fold(
            ProfilePolicy::new(self.inputs.profile.clone()),
            |policy, change| {
                policy
                    .apply(change)
                    .expect("a change the policy takes")
                    .expect("a change that makes a revision")
            },
        )
    }
}

/// What a case hands sizing besides its policy snapshot, owned.
pub(crate) struct Given {
    pub(crate) instrument: FuturesInstrument,
    pub(crate) brackets: Vec<LeverageBracket>,
    pub(crate) usd_index: Option<UsdIndex>,
}

impl Given {
    pub(crate) fn of(fixture: &Fixture, inputs: &Inputs) -> Self {
        let brackets = match &inputs.brackets {
            Tiers::Of(key) => fixture
                .tiers
                .get(key)
                .unwrap_or_else(|| panic!("the fixture has no brackets for {key}"))
                .clone(),
            Tiers::Given(brackets) => brackets.clone(),
        };
        Self {
            instrument: fixture.listed(&inputs.instrument, inputs.rules_at_ms),
            brackets,
            usd_index: inputs.usd_index.as_ref().map(|index| UsdIndex {
                asset: index.asset.clone(),
                price: index.price,
                source: index.source.clone(),
                observed_at_ms: index.observed_at_ms,
            }),
        }
    }

    /// The case's request, for an entry admitted under `policy`.
    pub(crate) fn request<'a>(
        &'a self,
        inputs: &'a Inputs,
        policy: &'a PolicySnapshot,
    ) -> SizingRequest<'a> {
        SizingRequest {
            policy,
            side: inputs.side,
            expected_price: inputs.expected_price,
            signal_stop: inputs.signal_stop,
            instrument: &self.instrument,
            brackets: Observed {
                value: &self.brackets,
                at_ms: inputs.brackets_at_ms,
            },
            fees: Observed {
                value: inputs.fees,
                at_ms: inputs.fees_at_ms,
            },
            account: &inputs.account,
            reserved: &inputs.reserved,
            usd_index: self.usd_index.as_ref(),
            now_ms: inputs.now_ms,
            freshness: Default::default(),
        }
    }
}

fn amount(money: &Money) -> Value {
    Value::String(money.amount().to_string())
}

/// The figures sizing reached, under the names the Python model gives them.
fn put_figures(out: &mut Map<String, Value>, figures: &SizingFigures) {
    out.insert("budget".into(), amount(&figures.budget));
    if let Some(stop) = &figures.stop {
        out.insert("stop".into(), json!(stop.price.to_string()));
    }
    if let Some(position) = &figures.position {
        out.insert("qty".into(), json!(position.quantity.value().to_string()));
        out.insert("unit_loss".into(), amount(&position.unit_loss));
        out.insert("risk".into(), amount(&position.risk));
    }
    if let Some(notional) = &figures.notional {
        out.insert("notional".into(), amount(&notional.quote));
        out.insert("notional_settle".into(), amount(&notional.settle));
    }
    if let Some(margin) = &figures.margin {
        out.insert("initial_margin".into(), amount(&margin.initial));
        out.insert("fee_allowance".into(), amount(&margin.fees));
        out.insert("required_margin".into(), amount(&margin.required));
    }
    if let Some(cap) = &figures.bracket_cap {
        out.insert("bracket_cap".into(), amount(cap));
    }
    if let Some(pool) = &figures.pool {
        out.insert("pool".into(), json!(pool.required.asset().as_str()));
        out.insert("required_in_pool".into(), amount(&pool.required));
        out.insert("free_margin".into(), amount(&pool.free));
    }
}

fn put_fault(out: &mut Map<String, Value>, fault: &SizingFault) {
    out.insert("status".into(), json!("fault"));
    out.insert(
        "fault".into(),
        json!({
            "kind": fault.kind().as_str(),
            "input": fault.input().map(|input| input.as_str()),
        }),
    );
}

fn put_refusal(out: &mut Map<String, Value>, reason: &str) {
    out.insert("status".into(), json!("refused"));
    out.insert("reason".into(), json!(reason));
}

/// An answer's first lines: the settlement asset, and no warning yet.
pub(crate) fn opening(settle: &Asset) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("settle".into(), json!(settle.as_str()));
    out.insert("warnings".into(), json!([]));
    out
}

/// What sizing answers for a case's request, and then what its prices
/// say where it brings some, in the Python model's own shape. The sized
/// entry comes back too, for a test that goes on to its fills.
pub(crate) fn answered(
    inputs: &Inputs,
    given: &Given,
    policy: &PolicySnapshot,
) -> (Value, Option<SizedEntry>) {
    let mut out = opening(&given.instrument.id().settle);
    let entry = match given.request(inputs, policy).size() {
        Err(fault) => {
            put_fault(&mut out, &fault);
            return (Value::Object(out), None);
        }
        Ok(Sizing::Refused(refusal)) => {
            put_figures(&mut out, &refusal.figures);
            put_refusal(&mut out, refusal.refusal.as_str());
            return (Value::Object(out), None);
        }
        Ok(Sizing::Sized(entry)) => *entry,
    };
    put_figures(&mut out, &entry.figures());
    let warnings: Vec<&str> = entry
        .warnings()
        .iter()
        .map(|warning| warning.as_str())
        .collect();
    out.insert("warnings".into(), json!(warnings));
    let Some(prices) = &inputs.prices else {
        out.insert("status".into(), json!("sized"));
        return (Value::Object(out), Some(entry));
    };
    match entry.at_submission(prices, inputs.now_ms, &Default::default()) {
        Err(fault) => put_fault(&mut out, &fault),
        Ok(SubmissionCheck::Refused(refusal)) => {
            if let Some(risk) = &refusal.risk_at_fill {
                out.insert("risk_at_fill".into(), amount(risk));
            }
            put_refusal(&mut out, refusal.refusal.as_str());
        }
        Ok(SubmissionCheck::Clear(estimate)) => {
            out.insert("risk_at_fill".into(), amount(&estimate.risk));
            out.insert("status".into(), json!("sized"));
            return (Value::Object(out), Some(entry));
        }
    }
    (Value::Object(out), None)
}

/// The answer for an entry the profile's policy refuses before sizing.
pub(crate) fn refused_by_policy(settle: &Asset, reason: &str) -> Value {
    let mut out = opening(settle);
    put_refusal(&mut out, reason);
    Value::Object(out)
}
