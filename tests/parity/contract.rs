//! The futures contract's types against the matrix: the three state
//! machines ALG-733 implements, the refusal vocabulary, the axes, and the
//! policy values the types carry. A name changed on either side fails here.

use std::collections::BTreeSet;

use quantforge::model::futures::{
    AccountKind, AccountingStatus, IntentState, MarginMode, OwnerTag, PROPAGATION_DELAY_MS,
    PositionMode, ProfileId, ProfileIdentity, ProtectionState, SETTLE_WINDOW_MS, UNIVERSE_RULE,
};
use quantforge::model::{
    AccountRef, Category, Environment, PositionSide, Refusal, Venue, VenueAccountId,
};

use crate::model::{self, Machine};

/// A machine by its names: states, initial state, terminal states, and its
/// moves with how many the list holds, so a repeated move shows too.
#[derive(Debug, PartialEq, Eq)]
struct Shape {
    states: BTreeSet<String>,
    initial: String,
    terminal: BTreeSet<String>,
    transitions: BTreeSet<(String, String)>,
    listed: usize,
}

fn matrix_shape(machine: &Machine) -> Shape {
    let owned = |names: &[String]| names.iter().cloned().collect::<BTreeSet<_>>();
    Shape {
        states: owned(&machine.states),
        initial: machine.initial.clone(),
        terminal: owned(&machine.terminal),
        transitions: machine
            .edges()
            .into_iter()
            .flatten()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .collect(),
        listed: machine.transitions.len(),
    }
}

fn code_shape<S: Copy>(
    all: &[S],
    initial: S,
    is_terminal: fn(S) -> bool,
    transitions: &[(S, S)],
    name: fn(S) -> &'static str,
) -> Shape {
    Shape {
        states: all.iter().map(|&state| name(state).to_string()).collect(),
        initial: name(initial).to_string(),
        terminal: all
            .iter()
            .filter(|&&state| is_terminal(state))
            .map(|&state| name(state).to_string())
            .collect(),
        transitions: transitions
            .iter()
            .map(|&(from, to)| (name(from).to_string(), name(to).to_string()))
            .collect(),
        listed: transitions.len(),
    }
}

fn machine(name: &str) -> Shape {
    let matrix = model::load_matrix();
    let machine = matrix
        .states
        .get(name)
        .unwrap_or_else(|| panic!("the matrix has no {name} machine"));
    matrix_shape(machine)
}

#[test]
fn the_intent_states_are_the_matrix_signal_intent_machine() {
    let code = code_shape(
        &IntentState::ALL,
        IntentState::INITIAL,
        IntentState::is_terminal,
        &IntentState::TRANSITIONS,
        IntentState::as_str,
    );
    assert_eq!(code, machine("signal_intent"));
}

#[test]
fn the_protection_states_are_the_matrix_futures_position_machine() {
    let code = code_shape(
        &ProtectionState::ALL,
        ProtectionState::INITIAL,
        ProtectionState::is_terminal,
        &ProtectionState::TRANSITIONS,
        ProtectionState::as_str,
    );
    assert_eq!(code, machine("futures_position"));
}

#[test]
fn the_accounting_statuses_are_the_matrix_accounting_machine() {
    let code = code_shape(
        &AccountingStatus::ALL,
        AccountingStatus::INITIAL,
        AccountingStatus::is_terminal,
        &AccountingStatus::TRANSITIONS,
        AccountingStatus::as_str,
    );
    assert_eq!(code, machine("accounting"));
}

#[test]
fn the_refusals_are_the_matrix_refusal_vocabulary_in_its_order() {
    let matrix = model::load_matrix();
    let code: Vec<&str> = Refusal::ALL
        .iter()
        .map(|refusal| refusal.as_str())
        .collect();
    assert_eq!(matrix.errors.refusal, code);
}

#[test]
fn the_venue_family_and_side_axes_are_the_code_enums() {
    let matrix = model::load_matrix();
    let venues: Vec<&str> = Venue::ALL.iter().map(|venue| venue.as_str()).collect();
    let families: Vec<&str> = Category::ALL.iter().map(|family| family.as_str()).collect();
    let sides: Vec<&str> = PositionSide::ALL.iter().map(|side| side.as_str()).collect();
    assert_eq!(matrix.axes.venue, venues);
    assert_eq!(matrix.axes.family, families);
    assert_eq!(matrix.axes.side, sides);
}

#[test]
fn the_policy_environments_and_modes_are_the_code_enums() {
    let matrix = model::load_matrix();
    let policy = &matrix.policy;
    let environments: Vec<&str> = Environment::ALL.iter().map(|env| env.as_str()).collect();
    assert_eq!(policy.environments, environments);
    assert_eq!(policy.position_mode, PositionMode::OneWay.as_str());
    let margins: Vec<&str> = MarginMode::ALL.iter().map(|mode| mode.as_str()).collect();
    assert_eq!(policy.margin_modes, margins);
    let refused = [
        PositionMode::Hedge.as_str(),
        AccountKind::PortfolioMargin.as_str(),
        AccountKind::BybitUnifiedV1.as_str(),
    ];
    assert_eq!(policy.refused_modes, refused);
}

/// The keys of `value`'s JSON object, sorted.
fn keys(value: serde_json::Value) -> BTreeSet<String> {
    match value {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        other => panic!("expected a JSON object, got {other}"),
    }
}

#[test]
fn account_and_profile_identities_carry_exactly_the_policy_fields() {
    let matrix = model::load_matrix();
    let id = VenueAccountId::issued("12345").expect("account id");
    let account = AccountRef::new(Venue::Bybit, Environment::Demo, id).expect("account");
    let profile = ProfileIdentity::new(
        ProfileId::new("main").expect("profile id"),
        account.clone(),
        Category::Linear,
        OwnerTag::new("k3x9p2").expect("owner tag"),
    )
    .expect("profile identity");
    let policy = &matrix.policy;
    let expected = |names: &[String]| names.iter().cloned().collect::<BTreeSet<_>>();
    let account_keys = keys(serde_json::to_value(&account).expect("account json"));
    assert_eq!(account_keys, expected(&policy.account_ref));
    let profile_keys = keys(serde_json::to_value(&profile).expect("profile json"));
    assert_eq!(profile_keys, expected(&policy.profile_identity));
}

#[test]
fn the_unknown_outcome_bounds_and_the_universe_rule_are_the_code_constants() {
    let matrix = model::load_matrix();
    let signals = &matrix.policy.signals;
    assert_eq!(
        i64::from(signals.propagation_delay_s) * 1000,
        PROPAGATION_DELAY_MS
    );
    assert_eq!(i64::from(signals.settle_window_s) * 1000, SETTLE_WINDOW_MS);
    assert_eq!(matrix.universe.rule_version, UNIVERSE_RULE);
}
