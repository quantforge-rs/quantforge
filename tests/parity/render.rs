//! `matrix.md`: the matrix rendered for people, generated from the TOML and
//! compared byte for byte.

use std::collections::BTreeMap;

use crate::cells::{self, Cell};
use crate::model::{self, Capability, Keyed, Matrix, RENDERED};
use crate::snapshot;

/// Text safe inside a table cell.
fn cell_text(text: &str) -> String {
    text.replace('|', "\\|")
}

fn joined(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn keyed(keyed: &Keyed) -> String {
    keyed
        .iter()
        .map(|(key, entries)| format!("{key}: {}", entries.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

fn yes_no(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

struct Page(String);

impl Page {
    fn line(&mut self, text: impl AsRef<str>) {
        self.0.push_str(text.as_ref());
        self.0.push('\n');
    }

    fn blank(&mut self) {
        self.0.push('\n');
    }

    fn table(&mut self, header: &[&str], rows: Vec<Vec<String>>) {
        self.line(format!("| {} |", header.join(" | ")));
        self.line(format!("|{}", " --- |".repeat(header.len())));
        for row in rows {
            let cells: Vec<String> = row.iter().map(|text| cell_text(text)).collect();
            self.line(format!("| {} |", cells.join(" | ")));
        }
        self.blank();
    }
}

fn capability_block(page: &mut Page, capability: &Capability, cells: &[&Cell]) {
    page.line(format!("### `{}`", capability.id));
    page.blank();
    page.line(format!("{}.", capability.title));
    page.blank();
    page.line(format!(
        "- **Cells:** {}, over {}",
        cells.len(),
        capability.axes.join(" × ")
    ));
    for (axis, values) in &capability.only {
        page.line(format!(
            "- **Only {axis}:** {}. {}",
            values.join(", "),
            capability.reason
        ));
    }
    if capability.refusal {
        page.line("- **A refusal:** the cell passes when the request is refused");
    }
    page.line(format!("- **Passes when:** {}", capability.passes));
    page.line(format!(
        "- **Evidence required:** {}",
        capability.requires.join(", ")
    ));
    page.line(format!(
        "- **Implementation:** {}",
        keyed(&capability.implementation)
    ));
    page.line(format!("- **Evidence:** {}", keyed(&capability.evidence)));
    if !capability.flags.is_empty() {
        page.line(format!(
            "- **Capability flags:** {}",
            keyed(&capability.flags)
        ));
    }
    if !capability.requirements.is_empty() {
        page.line(format!(
            "- **Requirements:** {}",
            capability.requirements.join(", ")
        ));
    }
    page.line(format!("- **Revised in:** revision {}", capability.revised));
    page.blank();
}

fn transitions(machine: &model::Machine) -> Vec<String> {
    let mut by_source: Vec<(&str, Vec<&str>)> = Vec::new();
    for (from, to) in machine.edges().into_iter().flatten() {
        match by_source.iter_mut().find(|(source, _)| *source == from) {
            Some((_, targets)) => targets.push(to),
            None => by_source.push((from, vec![to])),
        }
    }
    by_source
        .into_iter()
        .map(|(from, targets)| format!("  - {from} → {}", targets.join(", ")))
        .collect()
}

pub fn render(matrix: &Matrix) -> String {
    let cells = cells::expand(matrix);
    let listing = model::load_listing(matrix);
    let mut page = Page(String::new());

    page.line("# Capability parity matrix");
    page.blank();
    page.line("<!-- Generated from matrix.toml by `UPDATE_SNAPSHOTS=1 cargo test --test parity`. Edit matrix.toml, never this page. -->");
    page.blank();
    page.line(format!(
        "This page renders [matrix.toml](matrix.toml): revision {}, frozen on {} for QuantForge {}. [README.md](README.md) explains how to read it and keep it.",
        matrix.revision, matrix.source_date, matrix.release
    ));
    page.blank();
    page.line(format!(
        "It requires **{} cells** from {} capabilities in {} areas.",
        cells.len(),
        matrix.capabilities.len(),
        matrix.areas.len()
    ));
    page.blank();

    page.line("## Product cells");
    page.blank();
    page.line("These are the work order's 12 base product cells. A cell counts in each product cell its venue, store and family fit; a cell that names no venue or no store counts for each.");
    page.blank();
    let rows = cells::rollup(matrix, &cells)
        .into_iter()
        .map(|(venue, backend, family, count)| vec![venue, backend, family, count.to_string()])
        .collect();
    page.table(&["Venue", "Store", "Family", "Cells"], rows);
    let familyless = cells
        .iter()
        .filter(|cell| cell.value("family").is_none())
        .count();
    page.line(format!(
        "Another {familyless} cells name no family: they cover stores, surfaces and artifacts."
    ));
    page.blank();

    let mut by_capability: BTreeMap<&str, Vec<&Cell>> = BTreeMap::new();
    for cell in &cells {
        by_capability
            .entry(cell.capability.as_str())
            .or_default()
            .push(cell);
    }
    for area in &matrix.areas {
        page.line(format!("## {}", area.title));
        page.blank();
        page.line(format!("The `{}` area.", area.id));
        page.blank();
        page.line(format!("- **Inputs:** {}", area.inputs));
        page.line(format!("- **Outputs:** {}", area.outputs));
        page.line(format!("- **States:** {}", joined(&area.states)));
        page.line(format!("- **Errors:** {}", joined(&area.errors)));
        page.line(format!("- **Units:** {}", joined(&area.units)));
        page.line(format!("- **Data quality:** {}", area.quality));
        page.line(format!("- **Persistence:** {}", area.persistence));
        page.line(format!("- **Compared:** {}", area.compare));
        page.blank();
        for capability in matrix
            .capabilities
            .iter()
            .filter(|capability| capability.area == area.id)
        {
            let mine = by_capability
                .get(capability.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            capability_block(&mut page, capability, mine);
        }
    }

    page.line("## State machines");
    page.blank();
    for (name, machine) in &matrix.states {
        page.line(format!("### `{name}`"));
        page.blank();
        page.line(format!("- **States:** {}", machine.states.join(", ")));
        page.line(format!("- **Initial:** {}", machine.initial));
        if machine.terminal.is_empty() {
            page.line("- **Terminal:** none; the machine cycles");
        } else {
            page.line(format!("- **Terminal:** {}", machine.terminal.join(", ")));
        }
        page.line("- **Transitions:**");
        for line in transitions(machine) {
            page.line(line);
        }
        page.blank();
    }

    page.line("## Vocabularies");
    page.blank();
    let rows = matrix
        .errors
        .named()
        .into_iter()
        .map(|(name, values)| vec![name.to_string(), values.join(", ")])
        .collect();
    page.table(&["Errors", "Names"], rows);
    let rows = matrix
        .units
        .iter()
        .map(|(unit, meaning)| vec![unit.clone(), meaning.clone()])
        .collect();
    page.table(&["Unit", "Meaning"], rows);

    let policy = &matrix.policy;
    page.line("## Policy");
    page.blank();
    page.line(format!("From {}.", policy.reference));
    page.blank();
    page.table(
        &["Setting", "Value"],
        vec![
            vec!["Environments".to_string(), policy.environments.join(", ")],
            vec![
                "Profile identity".to_string(),
                policy.profile_identity.join(", "),
            ],
            vec![
                "Account reference".to_string(),
                policy.account_ref.join(", "),
            ],
            vec!["Position mode".to_string(), policy.position_mode.clone()],
            vec!["Margin modes".to_string(), policy.margin_modes.join(", ")],
            vec!["Refused modes".to_string(), policy.refused_modes.join(", ")],
            vec![
                "Changes account settings".to_string(),
                yes_no(policy.changes_account_settings).to_string(),
            ],
        ],
    );
    for rule in &policy.rules {
        page.line(format!("- {rule}."));
    }
    page.blank();
    let signals = &policy.signals;
    page.line("### Signals");
    page.blank();
    page.line(format!("From {}.", signals.reference));
    page.blank();
    page.table(
        &["Setting", "Value"],
        vec![
            vec![
                "Alert age limit".to_string(),
                format!(
                    "{} s by default, settable from {} to {} s",
                    signals.age_limit_default_s, signals.age_limit_min_s, signals.age_limit_max_s
                ),
            ],
            vec![
                "Future allowance".to_string(),
                format!("{} s", signals.future_allowance_s),
            ],
            vec![
                "Drift limit".to_string(),
                format!("{} × R", signals.drift_limit),
            ],
            vec![
                "Propagation delay".to_string(),
                format!("{} s", signals.propagation_delay_s),
            ],
            vec![
                "Settle window".to_string(),
                format!("{} s", signals.settle_window_s),
            ],
            vec![
                "Stop repair".to_string(),
                format!(
                    "{} attempts within {} s",
                    signals.repair_attempts, signals.attempt_window_s
                ),
            ],
            vec![
                "Protective close".to_string(),
                format!(
                    "{} attempts within {} s",
                    signals.protective_close_attempts, signals.attempt_window_s
                ),
            ],
            vec![
                "Unprotected alerts".to_string(),
                format!("every {} s", signals.unprotected_alert_interval_s),
            ],
            vec![
                "Operator stop-move window".to_string(),
                format!("{} s", signals.operator_move_window_s),
            ],
        ],
    );
    let evidence = &policy.evidence;
    page.line("### Evidence");
    page.blank();
    page.line(format!("From {}.", evidence.reference));
    page.blank();
    page.table(
        &["Setting", "Value"],
        vec![
            vec![
                "Live environments".to_string(),
                keyed(&evidence.environments),
            ],
            vec![
                "PostgreSQL versions".to_string(),
                evidence.postgres_versions.join(", "),
            ],
            vec!["Artifacts".to_string(), evidence.artifacts.clone()],
        ],
    );

    let universe = &matrix.universe;
    page.line("## Product universe");
    page.blank();
    page.line(format!(
        "Rule {} ({}), recorded on {} in [{}]({}). The list is what the rule selected that day, never an allowlist.",
        universe.rule_version, universe.reference, listing.fetched_at, universe.list, universe.list
    ));
    page.blank();
    let rows = universe
        .rule
        .iter()
        .map(|rule| {
            vec![
                rule.venue.clone(),
                rule.family.clone(),
                rule.selects.clone(),
            ]
        })
        .collect();
    page.table(&["Venue", "Family", "Selects"], rows);
    let rows = listing
        .sources
        .iter()
        .map(|source| {
            vec![
                source.venue.clone(),
                source.category.clone(),
                source.environment.clone(),
                source.symbols.len().to_string(),
            ]
        })
        .collect();
    page.table(&["Venue", "Family", "Environment", "Perpetuals"], rows);

    page.line("## Artifacts");
    page.blank();
    let rows = matrix
        .artifacts
        .iter()
        .map(|artifact| {
            vec![
                format!("`{}`: {}", artifact.id, artifact.title),
                artifact.surfaces.join(", "),
                artifact.backends.join(", "),
                yes_no(artifact.published).to_string(),
                joined(&artifact.platforms),
                artifact.requirement.clone(),
            ]
        })
        .collect();
    page.table(
        &[
            "Artifact",
            "Surfaces",
            "Backends",
            "Published",
            "Platforms",
            "Requirement",
        ],
        rows,
    );

    page.line("## Optional values");
    page.blank();
    let rows = matrix
        .optional_values
        .iter()
        .map(|value| {
            vec![
                value.venue.clone(),
                value.field.clone(),
                value.absent_when.clone(),
                value.representation.clone(),
                value.source.clone(),
            ]
        })
        .collect();
    page.table(
        &["Venue", "Value", "Absent when", "Shown as", "Source"],
        rows,
    );

    page.line("## Blockers");
    page.blank();
    let rows = matrix
        .blockers
        .iter()
        .map(|blocker| {
            let cells: Vec<String> = blocker
                .cells
                .iter()
                .map(|(axis, value)| format!("{axis} {value}"))
                .collect();
            vec![
                format!("[{}](../futures-venues.md#unverified-items)", blocker.item),
                format!("`{}`", blocker.capability),
                cells.join(", "),
                blocker.effect.clone(),
            ]
        })
        .collect();
    page.table(&["Item", "Capability", "Cells", "Effect"], rows);

    page.line("## Scope changes");
    page.blank();
    let rows = matrix
        .scope_changes
        .iter()
        .map(|change| {
            vec![
                change.id.clone(),
                change.kind.as_str().to_string(),
                change.date.clone(),
                change.decided_by.clone(),
                change.summary.clone(),
                change.reference.clone(),
                joined(&change.removes),
            ]
        })
        .collect();
    page.table(
        &[
            "ID",
            "Kind",
            "Date",
            "By",
            "Summary",
            "Reference",
            "Removes",
        ],
        rows,
    );

    page.line("## Stories");
    page.blank();
    page.line("Cells each story owns as implementation or evidence. Consumers read the evidence and own none.");
    page.blank();
    let capabilities: BTreeMap<&str, &Capability> = matrix
        .capabilities
        .iter()
        .map(|capability| (capability.id.as_str(), capability))
        .collect();
    let rows = matrix
        .stories
        .iter()
        .map(|(story, role)| {
            let owns = |pick: fn(&Capability) -> &Keyed| {
                cells
                    .iter()
                    .filter(|cell| {
                        cells::resolve(pick(capabilities[cell.capability.as_str()]), cell)
                            .contains(story)
                    })
                    .count()
            };
            let consumer = matrix.consumers.contains(story);
            vec![
                story.clone(),
                role.clone(),
                owns(|capability| &capability.implementation).to_string(),
                if consumer {
                    "consumer".to_string()
                } else {
                    owns(|capability| &capability.evidence).to_string()
                },
            ]
        })
        .collect();
    page.table(&["Story", "Role", "Implements", "Evidences"], rows);

    let mut text = page.0;
    while text.ends_with("\n\n") {
        text.pop();
    }
    text
}

#[test]
fn the_rendered_matrix_matches_the_toml() {
    let matrix = model::load_matrix();
    snapshot::assert_generated(RENDERED, &render(&matrix));
}
