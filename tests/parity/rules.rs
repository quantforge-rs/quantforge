//! Structural rules: owners, parity, domain applicability, references and
//! traceability. Each test collects every problem it finds before failing.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;

use crate::cells::{self, Cell};
use crate::model::{self, Capability, EvidenceKind, Matrix, ScopeKind};

fn report(what: &str, problems: Vec<String>) {
    assert!(problems.is_empty(), "{what}:\n{}", problems.join("\n"));
}

pub fn is_date(text: &str) -> bool {
    text.len() == 10
        && text.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

fn is_story(id: &str) -> bool {
    id.strip_prefix("ALG-").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// The first column of every markdown table row in `text` that `accept` takes.
fn first_column(text: &str, accept: impl Fn(&str) -> bool) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("| "))
        .filter_map(|rest| rest.split(" |").next())
        .filter(|id| accept(id))
        .map(str::to_string)
        .collect()
}

fn digits(text: &str, count: usize) -> bool {
    text.len() == count && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A requirement ID on the requirements page: OBS-R001, D01 or P-001.
fn is_requirement(id: &str) -> bool {
    id.strip_prefix("OBS-R").is_some_and(|rest| digits(rest, 3))
        || id.strip_prefix('D').is_some_and(|rest| digits(rest, 2))
        || id.strip_prefix("P-").is_some_and(|rest| digits(rest, 3))
}

/// Whether `text` names `id` on its own, so that SC-1 does not match SC-10.
fn names(text: &str, id: &str) -> bool {
    text.match_indices(id)
        .any(|(at, _)| !text[at + id.len()..].starts_with(|next: char| next.is_ascii_digit()))
}

fn by_id(matrix: &Matrix) -> BTreeMap<&str, &Capability> {
    matrix
        .capabilities
        .iter()
        .map(|capability| (capability.id.as_str(), capability))
        .collect()
}

/// The values `capability` may take on `axis`.
fn allowed<'m>(matrix: &'m Matrix, capability: &'m Capability, axis: &str) -> &'m [String] {
    capability
        .only
        .get(axis)
        .map(Vec::as_slice)
        .or_else(|| matrix.axes.values(axis))
        .unwrap_or_default()
}

#[test]
fn the_matrix_declares_its_format_revision_and_date() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    if matrix.format != model::MATRIX_FORMAT {
        problems.push(format!(
            "format is {}, not {}",
            matrix.format,
            model::MATRIX_FORMAT
        ));
    }
    if matrix.revision == 0 {
        problems.push("revisions start at 1".to_string());
    }
    if !is_date(&matrix.source_date) {
        problems.push(format!(
            "source_date {:?} is not YYYY-MM-DD",
            matrix.source_date
        ));
    }
    if matrix.release.trim().is_empty() {
        problems.push("release is empty".to_string());
    }
    for capability in &matrix.capabilities {
        if capability.revised == 0 || capability.revised > matrix.revision {
            problems.push(format!(
                "{} is revised at {}, outside 1..={}",
                capability.id, capability.revised, matrix.revision
            ));
        }
    }
    report("the matrix header", problems);
}

#[test]
fn axis_values_are_unique_across_axes() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for axis in model::AXES {
        let values = matrix.axes.values(axis).expect("a declared axis");
        if values.is_empty() {
            problems.push(format!("axis {axis} has no values"));
        }
        for value in values {
            if value == "all" || value.contains(['.', '/', ' ']) {
                problems.push(format!(
                    "{axis} value {value:?} is reserved or holds . / or a space"
                ));
            }
            if let Some(other) = seen.insert(value, axis) {
                problems.push(format!("{value} is a value of both {other} and {axis}"));
            }
        }
    }
    report("the axes", problems);
}

#[test]
fn capabilities_are_unique_and_belong_to_declared_areas() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    let areas: BTreeSet<&str> = matrix.areas.iter().map(|area| area.id.as_str()).collect();
    let mut ids = BTreeSet::new();
    for capability in &matrix.capabilities {
        if !ids.insert(capability.id.as_str()) {
            problems.push(format!("{} is declared twice", capability.id));
        }
        if capability.id.contains(['/', ' ']) || capability.id.is_empty() {
            problems.push(format!("{:?} is not a usable capability ID", capability.id));
        }
        if !areas.contains(capability.area.as_str()) {
            problems.push(format!(
                "{} belongs to undeclared area {}",
                capability.id, capability.area
            ));
        }
        if capability.title.trim().is_empty() || capability.passes.trim().is_empty() {
            problems.push(format!(
                "{} needs a title and a pass condition",
                capability.id
            ));
        }
        let mut spanned = BTreeSet::new();
        for axis in &capability.axes {
            if matrix.axes.values(axis).is_none() {
                problems.push(format!("{} spans unknown axis {axis}", capability.id));
            }
            if !spanned.insert(axis) {
                problems.push(format!("{} spans {axis} twice", capability.id));
            }
        }
        if capability.axes.is_empty() {
            problems.push(format!("{} spans no axis", capability.id));
        }
    }
    for area in &areas {
        if !matrix
            .capabilities
            .iter()
            .any(|capability| capability.area == *area)
        {
            problems.push(format!("area {area} has no capability"));
        }
    }
    report("the capabilities", problems);
}

#[test]
fn every_required_cell_has_an_implementation_and_an_evidence_owner() {
    let matrix = model::load_matrix();
    let capabilities = by_id(&matrix);
    let mut problems = Vec::new();
    for cell in cells::expand(&matrix) {
        let capability = capabilities[cell.capability.as_str()];
        if cells::resolve(&capability.implementation, &cell).is_empty() {
            problems.push(format!("{} has no implementation owner", cell.id()));
        }
        let evidence = cells::resolve(&capability.evidence, &cell);
        if evidence.iter().all(|story| story == "ALG-769") {
            problems.push(format!(
                "{} has no evidence owner besides the final gate",
                cell.id()
            ));
        }
    }
    report("cell owners", problems);
}

#[test]
fn owners_are_declared_stories_and_consumers_own_no_evidence() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    for (id, role) in &matrix.stories {
        if !is_story(id) || role.trim().is_empty() {
            problems.push(format!("story {id:?} needs an ALG-N ID and a role"));
        }
    }
    let consumers: BTreeSet<&str> = matrix.consumers.iter().map(String::as_str).collect();
    let mut used: BTreeSet<&str> = consumers.clone();
    for consumer in &consumers {
        if !matrix.stories.contains_key(*consumer) {
            problems.push(format!("consumer {consumer} is not a declared story"));
        }
    }
    for capability in &matrix.capabilities {
        for (table, keyed) in [
            ("implementation", &capability.implementation),
            ("evidence", &capability.evidence),
        ] {
            for (key, stories) in keyed {
                let unique: BTreeSet<&String> = stories.iter().collect();
                if stories.is_empty() || unique.len() != stories.len() {
                    problems.push(format!(
                        "{} {table}.{key} is empty or repeats a story",
                        capability.id
                    ));
                }
                for story in stories {
                    used.insert(story);
                    if !matrix.stories.contains_key(story) {
                        problems.push(format!("{} names undeclared story {story}", capability.id));
                    }
                    if table == "evidence" && consumers.contains(story.as_str()) {
                        problems.push(format!(
                            "{} has consumer {story} as an evidence owner",
                            capability.id
                        ));
                    }
                }
            }
        }
    }
    for story in matrix.stories.keys() {
        if !used.contains(story.as_str()) {
            problems.push(format!("{story} is declared but owns nothing"));
        }
    }
    report("the stories", problems);
}

#[test]
fn keys_name_values_of_the_axes_a_capability_spans() {
    let matrix = model::load_matrix();
    let all = cells::expand(&matrix);
    let mut problems = Vec::new();
    for capability in &matrix.capabilities {
        let mine: Vec<&Cell> = all
            .iter()
            .filter(|cell| cell.capability == capability.id)
            .collect();
        let tables = [
            ("implementation", &capability.implementation),
            ("evidence", &capability.evidence),
            ("flags", &capability.flags),
        ];
        for (table, keyed) in tables {
            for key in keyed.keys() {
                if !mine.iter().any(|cell| cells::key_matches(key, cell)) {
                    problems.push(format!(
                        "{} {table} key {key:?} applies to none of its cells",
                        capability.id
                    ));
                }
            }
        }
    }
    report("owner and flag keys", problems);
}

#[test]
fn capabilities_span_both_venues_and_both_stores() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    for capability in &matrix.capabilities {
        for (axis, values) in &capability.only {
            let declared = matrix.axes.values(axis).unwrap_or_default();
            if !capability.spans(axis) {
                problems.push(format!(
                    "{} narrows {axis}, which it does not span",
                    capability.id
                ));
            }
            match axis.as_str() {
                "venue" => problems.push(format!(
                    "{} narrows the venues; parity never does",
                    capability.id
                )),
                "backend" => problems.push(format!(
                    "{} narrows the stores; parity never does, and an artifact sets its own",
                    capability.id
                )),
                "surface" if capability.spans("artifact") => problems.push(format!(
                    "{} narrows the surfaces of artifacts, which take them from [[artifact]]",
                    capability.id
                )),
                _ => {}
            }
            if values.is_empty() || values.iter().any(|value| !declared.contains(value)) {
                problems.push(format!(
                    "{} only.{axis} names values outside the axis",
                    capability.id
                ));
            }
            if values.len() == declared.len() {
                problems.push(format!("{} only.{axis} narrows nothing", capability.id));
            }
        }
        if capability.only.is_empty() != capability.reason.trim().is_empty() {
            problems.push(format!(
                "{} must give a reason exactly when it narrows an axis",
                capability.id
            ));
        }
    }
    report("parity", problems);
}

#[test]
fn spot_never_gains_futures_semantics() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    for capability in &matrix.capabilities {
        if capability.spans("side") && !capability.spans("family") {
            problems.push(format!("{} has sides but no family", capability.id));
        }
        if !capability.spans("family") {
            continue;
        }
        let families = allowed(&matrix, capability, "family");
        let has = |family: &str| families.iter().any(|value| value == family);
        if has("linear") != has("inverse") {
            problems.push(format!(
                "{} must require linear and inverse together",
                capability.id
            ));
        }
        if capability.spans("side") && has("spot") {
            problems.push(format!(
                "{} gives Spot a side; Spot is long or flat",
                capability.id
            ));
        }
        let strategy = capability.area == "strategy";
        let futures = matches!(capability.area.as_str(), "futures" | "automation");
        if strategy && !capability.refusal && families != ["spot"] {
            problems.push(format!(
                "{} runs candle strategies, which take Spot series only",
                capability.id
            ));
        }
        if (futures || (strategy && capability.refusal)) && has("spot") {
            problems.push(format!("{} gives Spot futures semantics", capability.id));
        }
    }
    report("domain applicability", problems);
}

#[test]
fn areas_name_declared_states_errors_and_units() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    let vocabularies: BTreeMap<&str, &[String]> = matrix.errors.named().into_iter().collect();
    let mut ids = BTreeSet::new();
    let (mut states, mut errors, mut units) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for area in &matrix.areas {
        if !ids.insert(area.id.as_str()) {
            problems.push(format!("area {} is declared twice", area.id));
        }
        let texts = [
            &area.title,
            &area.inputs,
            &area.outputs,
            &area.quality,
            &area.persistence,
            &area.compare,
        ];
        if texts.iter().any(|text| text.trim().is_empty()) {
            problems.push(format!(
                "area {} leaves part of its contract empty",
                area.id
            ));
        }
        for state in &area.states {
            states.insert(state.as_str());
            if !matrix.states.contains_key(state) {
                problems.push(format!(
                    "area {} names undeclared state machine {state}",
                    area.id
                ));
            }
        }
        for error in &area.errors {
            errors.insert(error.as_str());
            if !vocabularies.contains_key(error.as_str()) {
                problems.push(format!(
                    "area {} names undeclared error vocabulary {error}",
                    area.id
                ));
            }
        }
        for unit in &area.units {
            units.insert(unit.as_str());
            if !matrix.units.contains_key(unit) {
                problems.push(format!("area {} names undeclared unit {unit}", area.id));
            }
        }
    }
    for machine in matrix.states.keys() {
        if !states.contains(machine.as_str()) {
            problems.push(format!("state machine {machine} belongs to no area"));
        }
    }
    for (name, values) in &vocabularies {
        let unique: BTreeSet<&String> = values.iter().collect();
        if !errors.contains(name) || values.is_empty() || unique.len() != values.len() {
            problems.push(format!(
                "error vocabulary {name} is unused, empty or repeats a name"
            ));
        }
    }
    for (unit, meaning) in &matrix.units {
        if !units.contains(unit.as_str()) || meaning.trim().is_empty() {
            problems.push(format!("unit {unit} is unused or unexplained"));
        }
    }
    report("area contracts", problems);
}

#[test]
fn capabilities_require_known_evidence() {
    let matrix = model::load_matrix();
    let kinds: Vec<&str> = EvidenceKind::ALL.iter().map(|kind| kind.as_str()).collect();
    let mut problems = Vec::new();
    for capability in &matrix.capabilities {
        let unique: BTreeSet<&String> = capability.requires.iter().collect();
        if capability.requires.is_empty() || unique.len() != capability.requires.len() {
            problems.push(format!(
                "{} requires nothing, or a kind twice",
                capability.id
            ));
        }
        for kind in &capability.requires {
            if !kinds.contains(&kind.as_str()) {
                problems.push(format!(
                    "{} requires unknown evidence {kind}",
                    capability.id
                ));
            }
        }
        let requires = |kind: &str| capability.requires.iter().any(|required| required == kind);
        if capability.spans("backend") && !capability.spans("artifact") && !requires("real_store") {
            problems.push(format!(
                "{} spans the stores, so it needs real_store evidence",
                capability.id
            ));
        }
        if capability.spans("artifact") && !requires("artifact") {
            problems.push(format!(
                "{} spans artifacts, so it needs artifact evidence",
                capability.id
            ));
        }
    }
    report("required evidence", problems);
}

#[test]
fn every_automation_requirement_maps_to_a_capability() {
    let matrix = model::load_matrix();
    let page = first_column(&model::read(model::REQUIREMENTS), is_requirement);
    let mut problems = Vec::new();
    let mut mapped = BTreeSet::new();
    for capability in &matrix.capabilities {
        for requirement in &capability.requirements {
            mapped.insert(requirement.clone());
            if !page.contains(requirement) {
                problems.push(format!(
                    "{} maps {requirement}, which the requirements page lacks",
                    capability.id
                ));
            }
        }
        let unique: BTreeSet<&String> = capability.requirements.iter().collect();
        if unique.len() != capability.requirements.len() {
            problems.push(format!("{} maps a requirement twice", capability.id));
        }
    }
    for requirement in page.difference(&mapped) {
        problems.push(format!("{requirement} maps to no capability"));
    }
    if page.is_empty() {
        problems.push("the requirements page lists no requirement".to_string());
    }
    report("requirement coverage", problems);
}

#[test]
fn scope_changes_are_well_formed_and_logged_on_the_requirements_page() {
    let matrix = model::load_matrix();
    let capabilities = by_id(&matrix);
    let page = model::read(model::REQUIREMENTS);
    let mut problems = Vec::new();
    let mut ids = BTreeSet::new();
    for change in &matrix.scope_changes {
        let numbered = change
            .id
            .strip_prefix("SC-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !numbered || !ids.insert(change.id.as_str()) {
            problems.push(format!(
                "scope change {:?} needs a unique SC-N ID",
                change.id
            ));
        }
        if !is_date(&change.date) {
            problems.push(format!("{} has date {:?}", change.id, change.date));
        }
        let texts = [&change.decided_by, &change.summary, &change.reference];
        if texts.iter().any(|text| text.trim().is_empty()) {
            problems.push(format!(
                "{} must say who decided it, what and where",
                change.id
            ));
        }
        match change.kind {
            ScopeKind::Exclusion if !change.removes.is_empty() => problems.push(format!(
                "{} is an {}, which never had cells to remove",
                change.id,
                change.kind.as_str()
            )),
            ScopeKind::Reduction if change.removes.is_empty() => {
                problems.push(format!("{} is a reduction that removes nothing", change.id))
            }
            _ => {}
        }
        for removed in &change.removes {
            if !removed.contains('/')
                && !capabilities.contains_key(removed.as_str())
                && !removed.contains('.')
            {
                problems.push(format!(
                    "{} removes {removed:?}, which is neither a capability nor a cell",
                    change.id
                ));
            }
        }
        if !names(&page, &change.id) {
            problems.push(format!(
                "{} is not logged in {}",
                change.id,
                model::REQUIREMENTS
            ));
        }
    }
    report("scope changes", problems);
}

#[test]
fn blockers_name_unverified_venue_items_and_real_cells() {
    let matrix = model::load_matrix();
    let items = first_column(&model::read(model::VENUES), |id| {
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
    });
    let all = cells::expand(&matrix);
    let mut problems = Vec::new();
    for blocker in &matrix.blockers {
        if !items.contains(&blocker.item.to_string()) {
            problems.push(format!(
                "blocker item {} is not in {}'s unverified items",
                blocker.item,
                model::VENUES
            ));
        }
        if blocker.effect.trim().is_empty() {
            problems.push(format!("blocker {} has no effect", blocker.item));
        }
        let blocked = all.iter().filter(|cell| {
            cell.capability == blocker.capability
                && blocker
                    .cells
                    .iter()
                    .all(|(axis, value)| cell.value(axis) == Some(value.as_str()))
        });
        if blocked.count() == 0 {
            problems.push(format!(
                "blocker {} matches no cell of {}",
                blocker.item, blocker.capability
            ));
        }
    }
    report("blockers", problems);
}

#[test]
fn optional_values_cite_a_source_and_a_representation() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    for value in &matrix.optional_values {
        if !matrix.axes.venue.contains(&value.venue) {
            problems.push(format!(
                "optional value {} names venue {}",
                value.field, value.venue
            ));
        }
        let texts = [&value.field, &value.absent_when, &value.representation];
        if texts.iter().any(|text| text.trim().is_empty()) {
            problems.push(format!(
                "optional value {:?} must say when it is absent and how it shows",
                value.field
            ));
        }
        let file = value.source.split('#').next().unwrap_or_default();
        if file.is_empty() || !model::path(file).is_file() {
            problems.push(format!(
                "optional value {} cites {:?}, which is not a file here",
                value.field, value.source
            ));
        }
    }
    report("optional values", problems);
}

#[test]
fn artifacts_declare_what_they_promise() {
    let matrix = model::load_matrix();
    let mut problems = Vec::new();
    let declared: Vec<&String> = matrix
        .artifacts
        .iter()
        .map(|artifact| &artifact.id)
        .collect();
    let axis: Vec<&String> = matrix.axes.artifact.iter().collect();
    if declared != axis {
        problems.push(format!(
            "[[artifact]] lists {declared:?}, but the axis has {axis:?}"
        ));
    }
    for artifact in &matrix.artifacts {
        let surfaces_ok = !artifact.surfaces.is_empty()
            && artifact
                .surfaces
                .iter()
                .all(|surface| matrix.axes.surface.contains(surface));
        let backends_ok = !artifact.backends.is_empty()
            && artifact
                .backends
                .iter()
                .all(|backend| matrix.axes.backend.contains(backend));
        if !surfaces_ok || !backends_ok {
            problems.push(format!(
                "{} must name known surfaces and backends",
                artifact.id
            ));
        }
        if artifact.published && artifact.backends.len() != matrix.axes.backend.len() {
            problems.push(format!(
                "{} is published, so it carries every backend",
                artifact.id
            ));
        }
        if artifact.published == artifact.platforms.is_empty() {
            problems.push(format!(
                "{} must list platforms exactly when it is published",
                artifact.id
            ));
        }
        if artifact.title.trim().is_empty() || artifact.requirement.trim().is_empty() {
            problems.push(format!("{} needs a title and a requirement", artifact.id));
        }
    }
    report("artifacts", problems);
}

#[test]
fn the_policy_keeps_the_adr_0002_choices() {
    let matrix = model::load_matrix();
    let policy = &matrix.policy;
    let signals = &policy.signals;
    let mut problems = Vec::new();
    if policy.environments != ["mainnet", "testnet", "demo"] {
        problems.push(format!("environments are {:?}", policy.environments));
    }
    if policy.position_mode != "one-way" || policy.changes_account_settings {
        problems
            .push("ADR 0002 §4: one-way mode, and account settings are never changed".to_string());
    }
    let lists = [
        &policy.profile_identity,
        &policy.account_ref,
        &policy.margin_modes,
        &policy.refused_modes,
        &policy.rules,
    ];
    if lists.iter().any(|list| list.is_empty()) || policy.reference.trim().is_empty() {
        problems.push("the policy leaves a choice empty".to_string());
    }
    let ordered = signals.age_limit_min_s <= signals.age_limit_default_s
        && signals.age_limit_default_s <= signals.age_limit_max_s
        && signals.propagation_delay_s < signals.settle_window_s;
    let positive = [
        signals.future_allowance_s,
        signals.repair_attempts,
        signals.protective_close_attempts,
        signals.attempt_window_s,
        signals.unprotected_alert_interval_s,
        signals.operator_move_window_s,
    ];
    if !ordered || positive.contains(&0) || signals.reference.trim().is_empty() {
        problems.push("the signal bounds are out of order or zero".to_string());
    }
    match signals.drift_limit.parse::<Decimal>() {
        Ok(limit) if limit > Decimal::ONE => {}
        _ => problems.push(format!(
            "drift limit {:?} must be a decimal above 1",
            signals.drift_limit
        )),
    }
    let evidence = &policy.evidence;
    for (key, environments) in &evidence.environments {
        let known = key.split('.').all(|part| {
            matrix
                .axes
                .venue
                .iter()
                .chain(&matrix.axes.family)
                .any(|value| value == part)
        });
        if !known
            || environments.is_empty()
            || environments
                .iter()
                .any(|env| !policy.environments.contains(env))
        {
            problems.push(format!("evidence environments {key:?} = {environments:?}"));
        }
    }
    let versions_ok = !evidence.postgres_versions.is_empty()
        && evidence
            .postgres_versions
            .iter()
            .all(|version| !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()));
    if !versions_ok || evidence.artifacts.trim().is_empty() || evidence.reference.trim().is_empty()
    {
        problems.push(
            "the evidence policy needs PostgreSQL versions, the artifact rule and a reference"
                .to_string(),
        );
    }
    for (venue, environments) in &matrix.universe.environments {
        if !matrix.axes.venue.contains(venue)
            || environments
                .iter()
                .any(|env| !policy.environments.contains(env))
        {
            problems.push(format!("universe environments {venue} = {environments:?}"));
        }
    }
    report("the policy", problems);
}
