//! The state machines and vocabularies: checked for shape, and against the
//! code that already implements them, so the two cannot drift apart.

use std::collections::{BTreeMap, BTreeSet};

use quantforge::protocol::ErrorCode;
use quantforge::protocol::jobs::JobState;
use quantforge::{ExchangeCapabilities, ExchangeErrorKind, OrderStatus, RunStatus};

use crate::model;

fn names(values: &[String]) -> BTreeSet<&str> {
    values.iter().map(String::as_str).collect()
}

#[test]
fn state_machines_are_connected_and_end_in_their_terminal_states() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    for (name, machine) in &matrix.states {
        let states = names(&machine.states);
        if states.len() != machine.states.len() {
            problems.push(format!("{name} declares a state twice"));
        }
        if !states.contains(machine.initial.as_str()) {
            problems.push(format!(
                "{name} starts in undeclared state {}",
                machine.initial
            ));
        }
        let terminal = names(&machine.terminal);
        if !terminal.is_subset(&states) {
            problems.push(format!("{name} has an undeclared terminal state"));
        }
        let mut next: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for (edge, transition) in machine.edges().into_iter().zip(&machine.transitions) {
            let Some((from, to)) = edge else {
                problems.push(format!("{name}: {transition:?} is not `from -> to`"));
                continue;
            };
            if !states.contains(from) || !states.contains(to) {
                problems.push(format!("{name}: {transition} names an undeclared state"));
            }
            if terminal.contains(from) {
                problems.push(format!("{name}: {transition} leaves terminal state {from}"));
            }
            if from == to || !seen.insert((from, to)) {
                problems.push(format!("{name}: {transition} loops or repeats"));
            }
            next.entry(from).or_default().push(to);
        }
        for state in &states {
            if !terminal.contains(state) && !next.contains_key(state) {
                problems.push(format!("{name}: {state} is neither terminal nor left"));
            }
        }
        let mut reached = BTreeSet::from([machine.initial.as_str()]);
        let mut frontier = vec![machine.initial.as_str()];
        while let Some(state) = frontier.pop() {
            for &to in next.get(state).map(Vec::as_slice).unwrap_or_default() {
                if reached.insert(to) {
                    frontier.push(to);
                }
            }
        }
        for state in states.difference(&reached) {
            problems.push(format!(
                "{name}: {state} cannot be reached from {}",
                machine.initial
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "state machines:\n{}",
        problems.join("\n")
    );
}

#[test]
fn spot_order_states_are_the_order_statuses() {
    let matrix = model::load_matrix();
    let machine = &matrix.states["spot_order"];
    let statuses = [
        OrderStatus::PendingNew,
        OrderStatus::New,
        OrderStatus::PartiallyFilled,
        OrderStatus::Filled,
        OrderStatus::Canceled,
        OrderStatus::Rejected,
        OrderStatus::Expired,
    ];
    let code: BTreeSet<&str> = statuses.iter().map(|status| status.as_str()).collect();
    assert_eq!(names(&machine.states), code);
    assert!(
        !names(&machine.states).contains(OrderStatus::Unknown.as_str()),
        "UNKNOWN marks a mapping failure, not a state"
    );
}

#[test]
fn strategy_run_states_are_the_run_statuses() {
    let matrix = model::load_matrix();
    let statuses = [
        RunStatus::Starting,
        RunStatus::Running,
        RunStatus::Stopped,
        RunStatus::Failed,
    ];
    let code: BTreeSet<&str> = statuses.iter().map(|status| status.as_str()).collect();
    assert_eq!(names(&matrix.states["strategy_run"].states), code);
}

#[test]
fn job_states_are_the_wire_job_kinds() {
    let matrix = model::load_matrix();
    let machine = &matrix.states["job"];
    for kind in &machine.states {
        // Every field a job state can carry; each kind ignores the rest.
        let json = format!(
            r#"{{"kind":"{kind}","started_at_ms":1,"finished_at_ms":2,"results":[],"error":{{"code":"internal","message":"m"}}}}"#
        );
        let state: JobState = serde_json::from_str(&json).expect("a job state parses");
        assert!(
            !matches!(state, JobState::Unrecognized),
            "{kind} is not a job kind"
        );
    }
    assert_eq!(machine.states.len(), 5, "JobState has five kinds");
}

#[test]
fn exchange_errors_are_the_error_kinds() {
    let matrix = model::load_matrix();
    let kinds = [
        ExchangeErrorKind::Transport,
        ExchangeErrorKind::InvalidResponse,
        ExchangeErrorKind::InvalidRequest,
        ExchangeErrorKind::MissingCredentials,
        ExchangeErrorKind::Unauthorized,
        ExchangeErrorKind::ClockSkew,
        ExchangeErrorKind::RateLimited,
        ExchangeErrorKind::Unavailable,
        ExchangeErrorKind::OutcomeUnknown,
        ExchangeErrorKind::UnknownSymbol,
        ExchangeErrorKind::UnknownOrder,
        ExchangeErrorKind::InsufficientBalance,
        ExchangeErrorKind::RuleViolation,
        ExchangeErrorKind::Rejected,
        ExchangeErrorKind::Other,
    ];
    let code: BTreeSet<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(names(&matrix.errors.exchange), code);
    assert_eq!(
        matrix.errors.exchange.len(),
        code.len(),
        "an exchange error is listed twice"
    );
}

#[test]
fn wire_errors_are_the_protocol_error_codes() {
    let matrix = model::load_matrix();
    let codes = [
        ErrorCode::InvalidRequest,
        ErrorCode::Unauthorized,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::UnsupportedVersion,
        ErrorCode::LimitExceeded,
        ErrorCode::RateLimited,
        ErrorCode::Unavailable,
        ErrorCode::Internal,
    ];
    let code: BTreeSet<String> = codes
        .iter()
        .map(
            |code| match serde_json::to_value(code).expect("an error code serializes") {
                serde_json::Value::String(name) => name,
                other => panic!("error code serialized as {other}"),
            },
        )
        .collect();
    let listed: BTreeSet<String> = matrix.errors.wire.iter().cloned().collect();
    assert_eq!(listed, code);
    assert_eq!(
        matrix.errors.wire.len(),
        code.len(),
        "a wire error is listed twice"
    );
}

/// What `capabilities` declares for a flag the matrix names, or `None` when
/// the name is no capability flag. Reading the fields keeps the names tied to
/// the struct: a renamed field stops this from compiling.
fn flag(capabilities: &ExchangeCapabilities, name: &str) -> Option<bool> {
    Some(match name {
        "orders.market_by_base_quantity" => capabilities.orders.market_by_base_quantity,
        "orders.market_by_quote_quantity" => capabilities.orders.market_by_quote_quantity,
        "orders.client_order_ids" => capabilities.orders.client_order_ids,
        "market_data.symbol_rules" => capabilities.market_data.symbol_rules,
        "market_data.server_time" => capabilities.market_data.server_time,
        "streams.klines" => capabilities.streams.klines,
        "streams.user_data" => capabilities.streams.user_data,
        _ => return None,
    })
}

#[test]
fn capability_flags_name_exchange_capabilities() {
    let matrix = model::load_matrix();
    let empty = ExchangeCapabilities::default();
    let mut problems = Vec::new();
    for capability in &matrix.capabilities {
        for names in capability.flags.values() {
            for name in names {
                match flag(&empty, name) {
                    Some(false) => {}
                    Some(true) => problems.push(format!("the empty descriptor declares {name}")),
                    None => problems.push(format!("{} needs unknown flag {name}", capability.id)),
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "capability flags:\n{}",
        problems.join("\n")
    );
}
