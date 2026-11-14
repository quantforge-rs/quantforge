//! Reading a body as a signal: every member is judged, and everything
//! wrong with the body is found before any of it is said.

use std::collections::BTreeSet;

use super::super::ExactDecimal;
use super::SIGNAL_VERSION;
use super::fields::{self, Field};
use super::json::{self, Raw};
use super::payload::{Signal, SignalOrder};
use super::problems::{SignalProblem, SignalProblemCode, SignalProblems};
use super::times::SignalTime;
use super::words::{SignalCategory, SignalKind, SignalSide};

/// The first whole number a double no longer tells from its neighbor: a
/// JSON integer at or past it is not one every reader holds.
const EXACT_INTEGERS: u64 = 1 << 53;

/// The values read so far, each `None` until its member is read and found
/// good.
#[derive(Default)]
struct Read {
    version: bool,
    kind: Option<SignalKind>,
    profile: Option<String>,
    source: Option<String>,
    ticker: Option<String>,
    exchange: Option<String>,
    category: Option<SignalCategory>,
    side: Option<SignalSide>,
    bar_time: Option<SignalTime>,
    fire_time: Option<SignalTime>,
    price: Option<ExactDecimal>,
    stop: Option<ExactDecimal>,
}

impl Read {
    /// Takes one member's value, or says what is wrong with it.
    fn take(&mut self, field: Field, value: Raw) -> Result<(), SignalProblem> {
        if field == Field::Version {
            return match value {
                Raw::Whole(version) if version == u64::from(SIGNAL_VERSION) => {
                    self.version = true;
                    Ok(())
                }
                Raw::Whole(version) if version < EXACT_INTEGERS => Err(SignalProblem::field(
                    SignalProblemCode::UnsupportedVersion,
                    field.name(),
                    format!(
                        "version {version} is not supported: this is version {SIGNAL_VERSION} \
                         of the format"
                    ),
                )),
                other => Err(wrong_type(field, &other)),
            };
        }
        let Raw::Text(text) = value else {
            return Err(wrong_type(field, &value));
        };
        let invalid = || {
            SignalProblem::field(
                SignalProblemCode::InvalidValue,
                field.name(),
                format!(
                    "{:?} is {text:?}, and must be {}",
                    field.name(),
                    field.rule()
                ),
            )
        };
        let named = |valid: bool| valid.then(|| text.clone()).ok_or_else(invalid);
        match field {
            Field::Version => {}
            Field::Kind => self.kind = Some(SignalKind::named(&text).ok_or_else(invalid)?),
            Field::Profile => self.profile = Some(named(fields::is_name(&text))?),
            Field::Source => self.source = Some(named(fields::is_name(&text))?),
            Field::Ticker => self.ticker = Some(named(fields::is_ticker(&text))?),
            Field::Exchange => self.exchange = Some(named(fields::is_exchange(&text))?),
            Field::Category => {
                self.category = Some(SignalCategory::named(&text).ok_or_else(invalid)?);
            }
            Field::Side => self.side = Some(SignalSide::named(&text).ok_or_else(invalid)?),
            Field::BarTime => self.bar_time = Some(SignalTime::parse(&text).ok_or_else(invalid)?),
            Field::FireTime => {
                self.fire_time = Some(SignalTime::parse(&text).ok_or_else(invalid)?);
            }
            Field::Price => self.price = Some(fields::price(&text).ok_or_else(invalid)?),
            Field::Stop => self.stop = Some(fields::price(&text).ok_or_else(invalid)?),
        }
        Ok(())
    }

    /// The signal, once every value its kind needs has been read.
    fn assemble(self) -> Option<Signal> {
        let order = match self.kind? {
            SignalKind::Entry => SignalOrder::Entry {
                price: self.price?,
                stop: self.stop,
            },
            SignalKind::Exit => SignalOrder::Exit,
        };
        self.version.then_some(())?;
        Some(Signal {
            profile: self.profile?,
            source: self.source?,
            ticker: self.ticker?,
            exchange: self.exchange,
            category: self.category,
            side: self.side?,
            bar_time: self.bar_time?,
            fire_time: self.fire_time?,
            order,
        })
    }
}

fn wrong_type(field: Field, value: &Raw) -> SignalProblem {
    SignalProblem::field(
        SignalProblemCode::WrongType,
        field.name(),
        format!(
            "{:?} is {}, and must be {}",
            field.name(),
            value.described(),
            field.json_type()
        ),
    )
}

/// The signal a body is, or everything that keeps it from being one.
pub(super) fn signal(body: &[u8]) -> Result<Signal, SignalProblems> {
    let members = json::members(body).map_err(|problem| SignalProblems::sorted(vec![problem]))?;
    // Each member as it was first written, and the names written again.
    let mut names = BTreeSet::new();
    let mut twice = BTreeSet::new();
    let mut first = Vec::with_capacity(members.len());
    for (name, value) in members {
        if names.insert(name.clone()) {
            first.push((name, value));
        } else {
            twice.insert(name);
        }
    }
    let mut problems: Vec<SignalProblem> = twice
        .iter()
        .map(|name| {
            SignalProblem::field(
                SignalProblemCode::DuplicateField,
                name,
                format!("{name:?} is written more than once"),
            )
        })
        .collect();
    // What the body is decides which members belong in it, wherever its
    // kind is written. A kind written twice, or one that is no kind, says
    // nothing.
    let kind = first
        .iter()
        .find(|(name, _)| name == Field::Kind.name() && !twice.contains(name))
        .and_then(|(_, value)| match value {
            Raw::Text(text) => SignalKind::named(text),
            _ => None,
        });
    let mut read = Read::default();
    for (name, value) in first {
        if twice.contains(&name) {
            continue;
        }
        let Some(field) = Field::named(&name) else {
            problems.push(SignalProblem::field(
                SignalProblemCode::UnknownField,
                &name,
                format!("{name:?} is not a field of a version {SIGNAL_VERSION} signal"),
            ));
            continue;
        };
        if kind == Some(SignalKind::Exit) && field.entry_only() {
            problems.push(SignalProblem::field(
                SignalProblemCode::MisplacedField,
                field.name(),
                format!(
                    "{:?} belongs to an entry, and this is an exit",
                    field.name()
                ),
            ));
            continue;
        }
        if let Err(problem) = read.take(field, value) {
            problems.push(problem);
        }
    }
    let entry = kind == Some(SignalKind::Entry);
    for field in Field::ALL {
        if field.required(entry) && !names.contains(field.name()) {
            problems.push(SignalProblem::field(
                SignalProblemCode::MissingField,
                field.name(),
                format!("{:?} is missing", field.name()),
            ));
        }
    }
    if let (Some(bar), Some(fire)) = (&read.bar_time, &read.fire_time) {
        if bar > fire {
            problems.push(SignalProblem::field(
                SignalProblemCode::BarAfterFire,
                Field::BarTime.name(),
                format!(
                    "\"bar_time\" {bar} is after \"fire_time\" {fire}: a bar opens before its \
                     alert fires"
                ),
            ));
        }
    }
    // A value that is not there has a problem of its own above: its member
    // is missing, written twice, misplaced or wrong. So the list is empty
    // only when the signal is whole.
    match read.assemble() {
        Some(signal) if problems.is_empty() => Ok(signal),
        _ => Err(SignalProblems::sorted(problems)),
    }
}
