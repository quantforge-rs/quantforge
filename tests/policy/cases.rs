//! The shared policy cases: the four product lines with their venue lists,
//! each case's steps with the answers the Python model recorded, and the
//! fixture's changes turned into the model's.

use std::path::Path;
use std::str::FromStr;

use quantforge::model::futures::{
    AssetEdit, DefaultsEdit, Edit, EffectivePolicy, FuturesInstrument, NewAsset, PolicyChange,
    ProfileIdentity, ProfileLabel, RiskPercent, Sourced, StopFallback, StopPercent,
};
use quantforge::model::{Asset, ExchangeId, InstrumentId, Leverage, Money, Price, Symbol};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fixture {
    pub(crate) format: u32,
    #[allow(dead_code, reason = "names the script that writes the fixture")]
    pub(crate) generated_by: String,
    pub(crate) policy_format: u32,
    pub(crate) snapshot_format: u32,
    pub(crate) lines: Vec<Line>,
    pub(crate) cases: Vec<Case>,
    pub(crate) races: Vec<Race>,
}

/// One product line: who its profile is, the symbols the cases use, and
/// the instruments its venue lists.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Line {
    pub(crate) name: String,
    pub(crate) identity: ProfileIdentity,
    #[serde(rename = "use")]
    pub(crate) symbols: Symbols,
    pub(crate) instruments: Vec<FuturesInstrument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Symbols {
    pub(crate) main: String,
    pub(crate) second: String,
    #[allow(dead_code, reason = "the steps name it themselves")]
    pub(crate) dated: String,
    #[allow(dead_code, reason = "the steps name it themselves")]
    pub(crate) halted: String,
    #[allow(dead_code, reason = "the steps name it themselves")]
    pub(crate) other_settle: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) doc: String,
    pub(crate) line: String,
    /// The policy the profile is created with, as stored.
    pub(crate) created: String,
    pub(crate) steps: Vec<Step>,
}

/// One step of a case: what is asked, and what the model answered.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    pub(crate) change: Option<Value>,
    pub(crate) document: Option<String>,
    pub(crate) unchanged: Option<bool>,
    pub(crate) refused: Option<Value>,
    pub(crate) effective: Option<String>,
    pub(crate) expect: Option<Value>,
    pub(crate) entry: Option<String>,
    pub(crate) signal_stop: Option<String>,
    pub(crate) snapshot: Option<String>,
    pub(crate) stop: Option<Value>,
    pub(crate) check: Option<bool>,
    pub(crate) problems: Option<Vec<Value>>,
}

/// Two changes made from one revision, with the documents of either order.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Race {
    pub(crate) id: String,
    pub(crate) doc: String,
    pub(crate) line: String,
    pub(crate) base: Vec<Value>,
    pub(crate) base_document: String,
    pub(crate) left: Value,
    pub(crate) right: Value,
    pub(crate) left_alone: String,
    pub(crate) right_alone: String,
    pub(crate) left_then_right: String,
    pub(crate) right_then_left: String,
}

pub(crate) fn fixture() -> Fixture {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("policy_cases.json");
    let text = std::fs::read_to_string(&path).expect("read tests/fixtures/policy_cases.json");
    serde_json::from_str(&text).expect("the policy cases")
}

impl Fixture {
    pub(crate) fn line(&self, name: &str) -> &Line {
        self.lines
            .iter()
            .find(|line| line.name == name)
            .unwrap_or_else(|| panic!("the fixture has no line {name}"))
    }
}

impl Line {
    /// The product line, as its instruments name it.
    pub(crate) fn exchange(&self) -> ExchangeId {
        self.instruments[0].id().exchange
    }

    /// The instrument the venue lists under `symbol`.
    pub(crate) fn listed(&self, symbol: &str) -> &FuturesInstrument {
        self.instruments
            .iter()
            .find(|listed| listed.id().symbol.as_str() == symbol)
            .unwrap_or_else(|| panic!("{} lists no {symbol}", self.name))
    }
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected text, not {value}"))
}

pub(crate) fn risk(value: &Value) -> RiskPercent {
    RiskPercent::new(decimal(text(value))).expect("a risk percentage")
}

fn stop_percent(value: &Value) -> StopPercent {
    StopPercent::new(decimal(text(value))).expect("a stop percentage")
}

fn leverage(value: &Value) -> Leverage {
    Leverage::new(decimal(text(value))).expect("a leverage")
}

pub(crate) fn price(value: &str) -> Price {
    Price::new(decimal(value)).expect("a price")
}

/// An edit of one optional setting: absent keeps it, `"clear"` removes
/// it, and `{"set": value}` sets it.
fn edit<T>(body: &Value, key: &str, read: fn(&Value) -> T) -> Edit<T> {
    match body.get(key) {
        None => Edit::Keep,
        Some(Value::String(word)) if word == "clear" => Edit::Clear,
        Some(value) => Edit::Set(read(&value["set"])),
    }
}

fn fallback(value: &Value) -> StopFallback {
    match value {
        Value::String(word) if word == "inherit" => StopFallback::Inherit,
        Value::String(word) if word == "off" => StopFallback::Off,
        value => StopFallback::Percent(stop_percent(&value["percent"])),
    }
}

/// The exchange a fixture names by its stored token, such as
/// `BybitInverse`.
fn exchange(token: &Value) -> ExchangeId {
    serde_json::from_value(token.clone()).expect("a product line")
}

/// A fixture's change as the model's, on `line`.
pub(crate) fn change(line: &Line, change: &Value) -> PolicyChange {
    let (kind, body) = change
        .as_object()
        .and_then(|object| object.iter().next())
        .unwrap_or_else(|| panic!("no change in {change}"));
    let symbol = |value: &Value| Symbol::new(text(value)).expect("a symbol");
    match kind.as_str() {
        "label" => PolicyChange::Label(
            body.as_str()
                .map(|label| ProfileLabel::new(label).expect("a label")),
        ),
        "defaults" => PolicyChange::Defaults(DefaultsEdit {
            enabled: body.get("enabled").and_then(Value::as_bool),
            risk_percent: edit(body, "risk_percent", risk),
            stop_percent: edit(body, "stop_percent", stop_percent),
            leverage: edit(body, "leverage", leverage),
        }),
        "add_asset" => PolicyChange::AddAsset(NewAsset {
            instrument: InstrumentId::new(
                body.get("exchange").map_or(line.exchange(), exchange),
                symbol(&body["symbol"]),
                Asset::new(text(&body["settle"])).expect("an asset"),
            ),
            enabled: body["enabled"].as_bool().expect("a switch"),
            risk_percent: body.get("risk_percent").map(risk),
            stop: body.get("stop").map_or(StopFallback::Inherit, fallback),
            leverage: body.get("leverage").map(leverage),
        }),
        "edit_asset" => PolicyChange::EditAsset {
            symbol: symbol(&body["symbol"]),
            edit: AssetEdit {
                enabled: body.get("enabled").and_then(Value::as_bool),
                risk_percent: edit(body, "risk_percent", risk),
                stop: body.get("stop").map(fallback),
                leverage: edit(body, "leverage", leverage),
            },
        },
        "reset_asset" => PolicyChange::ResetAsset {
            symbol: symbol(body),
        },
        "set_capital" => PolicyChange::SetCapital(Money::new(
            Decimal::from_str(text(&body["amount"])).expect("an amount"),
            Asset::new(text(&body["asset"])).expect("an asset"),
        )),
        "clear_capital" => PolicyChange::ClearCapital(Asset::new(text(body)).expect("an asset")),
        other => panic!("the fixture has a change this harness does not know: {other}"),
    }
}

fn sourced<T: ToString>(setting: Option<Sourced<T>>) -> Value {
    setting.map_or(Value::Null, |setting| {
        json!({"value": setting.value.to_string(), "source": format!("{:?}", setting.source)})
    })
}

/// What an instrument trades on, in the fixture's spelling: every decimal
/// as its exact text.
pub(crate) fn effective_json(effective: &EffectivePolicy) -> Value {
    let stop = &effective.stop_percent;
    json!({
        "revision": effective.revision.get(),
        "configured": effective.configured,
        "enabled": effective.enabled,
        "risk_percent": sourced(effective.risk_percent),
        "stop_percent": {
            "value": stop.value.map(|percent| percent.to_string()),
            "source": format!("{:?}", stop.source),
        },
        "leverage": sourced(effective.leverage),
        "capital": effective.capital.as_ref().map(|capital| json!({
            "amount": capital.amount().amount().to_string(),
            "asset": capital.asset().as_str(),
            "revision": capital.revision().get(),
        })),
    })
}
