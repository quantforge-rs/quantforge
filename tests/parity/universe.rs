//! The dated universe list: what rule 1 selected on its date, recorded once.
//! It is a record, never an allowlist, and its counts are never pinned.

use std::collections::{BTreeMap, BTreeSet};

use crate::model;

#[test]
fn the_universe_list_was_selected_by_the_frozen_rule() {
    let matrix = model::load_matrix();
    let listing = model::load_listing(&matrix);
    let universe = &matrix.universe;
    let mut problems = Vec::new();
    if listing.rule_version != universe.rule_version {
        problems.push(format!(
            "the list was selected by rule {}, the matrix freezes rule {}",
            listing.rule_version, universe.rule_version
        ));
    }
    let date = listing.fetched_at.get(..10).unwrap_or_default();
    let named = format!("universe-v{}-{date}.json", universe.rule_version);
    if universe.list != named
        || !listing.fetched_at.ends_with('Z')
        || listing.fetched_at.len() != 20
    {
        problems.push(format!(
            "{} was fetched at {}, so it is named {named}",
            universe.list, listing.fetched_at
        ));
    }
    let rules: BTreeMap<String, String> = universe
        .rule
        .iter()
        .map(|rule| {
            (
                format!("{} {}", rule.venue, rule.family),
                rule.selects.clone(),
            )
        })
        .collect();
    if rules.len() != universe.rule.len() {
        problems.push("a venue and family has two rules".to_string());
    }
    if rules != listing.rule {
        problems.push(format!(
            "the list's rule {:?} is not the matrix's {rules:?}",
            listing.rule
        ));
    }
    for venue in &matrix.axes.venue {
        for family in ["linear", "inverse"] {
            if !rules.contains_key(&format!("{venue} {family}")) {
                problems.push(format!(
                    "rule {} has no {venue} {family} predicate",
                    universe.rule_version
                ));
            }
        }
    }
    if universe.reference.trim().is_empty() {
        problems.push("the universe cites no decision".to_string());
    }
    assert!(
        problems.is_empty(),
        "the universe list:\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_required_family_has_trading_perpetuals_in_every_environment() {
    let matrix = model::load_matrix();
    let listing = model::load_listing(&matrix);
    let mut expected = BTreeSet::new();
    for (venue, environments) in &matrix.universe.environments {
        for family in ["linear", "inverse"] {
            for environment in environments {
                expected.insert((venue.as_str(), family, environment.as_str()));
            }
        }
    }
    let listed: BTreeSet<(&str, &str, &str)> = listing
        .sources
        .iter()
        .map(|source| {
            (
                source.venue.as_str(),
                source.category.as_str(),
                source.environment.as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed, expected,
        "the list covers exactly the required venue, family and environment"
    );
    for source in &listing.sources {
        assert!(
            !source.symbols.is_empty(),
            "{} {} {}: a required family without trading perpetuals blocks the release",
            source.venue,
            source.category,
            source.environment
        );
        assert!(
            source.url.starts_with("https://"),
            "{} is not an HTTPS source",
            source.url
        );
    }
}

#[test]
fn universe_symbols_are_trimmed_unique_and_in_code_point_order() {
    let matrix = model::load_matrix();
    let listing = model::load_listing(&matrix);
    for source in &listing.sources {
        for symbol in &source.symbols {
            assert!(
                !symbol.is_empty() && symbol.trim() == symbol,
                "{} {} {}: symbol {symbol:?}",
                source.venue,
                source.category,
                source.environment
            );
        }
        // UTF-8 byte order is code-point order, which is Python's sort order.
        for pair in source.symbols.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{} {}: {:?} before {:?}",
                source.venue,
                source.environment,
                pair[0],
                pair[1]
            );
        }
    }
}
