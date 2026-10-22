//! Cells: expanding capabilities over their axes, resolving each cell's
//! owners, and the append-only ledger in `cells.txt` that makes every
//! reduction of coverage name a recorded scope change.

use std::collections::BTreeSet;

use crate::model::{self, AXES, Capability, Keyed, LEDGER, Matrix, ScopeKind};
use crate::snapshot;

/// One required cell: a capability and a value for each axis it spans.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    pub capability: String,
    /// `(axis, value)` in [`AXES`] order.
    pub values: Vec<(&'static str, String)>,
}

impl Cell {
    pub fn id(&self) -> String {
        let mut id = self.capability.clone();
        for (_, value) in &self.values {
            id.push('/');
            id.push_str(value);
        }
        id
    }

    pub fn value(&self, axis: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| *name == axis)
            .map(|(_, value)| value.as_str())
    }

    pub fn has(&self, value: &str) -> bool {
        self.values.iter().any(|(_, held)| held == value)
    }
}

/// The values `capability` takes on `axis`, in axis order: narrowed by its
/// `only`, and for an artifact's surfaces and backends by that artifact.
fn axis_values(
    matrix: &Matrix,
    capability: &Capability,
    axis: &str,
    artifact: Option<&str>,
) -> Vec<String> {
    let declared = matrix
        .artifacts
        .iter()
        .find(|candidate| Some(candidate.id.as_str()) == artifact);
    let allowed: Option<&Vec<String>> = match (axis, declared) {
        ("surface", Some(declared)) => Some(&declared.surfaces),
        ("backend", Some(declared)) => Some(&declared.backends),
        _ => None,
    };
    matrix
        .axes
        .values(axis)
        .unwrap_or_default()
        .iter()
        .filter(|value| allowed.is_none_or(|allowed| allowed.contains(*value)))
        .filter(|value| {
            capability
                .only
                .get(axis)
                .is_none_or(|only| only.contains(*value))
        })
        .cloned()
        .collect()
}

/// Every required cell, sorted by ID.
pub fn expand(matrix: &Matrix) -> Vec<Cell> {
    let mut cells = Vec::new();
    for capability in &matrix.capabilities {
        let axes: Vec<&'static str> = AXES
            .iter()
            .copied()
            .filter(|axis| capability.spans(axis))
            .collect();
        let artifacts: Vec<Option<String>> = if capability.spans("artifact") {
            axis_values(matrix, capability, "artifact", None)
                .into_iter()
                .map(Some)
                .collect()
        } else {
            vec![None]
        };
        for artifact in &artifacts {
            let mut rows: Vec<Vec<(&'static str, String)>> = vec![Vec::new()];
            for &axis in &axes {
                let values = match artifact {
                    Some(artifact) if axis == "artifact" => vec![artifact.clone()],
                    _ => axis_values(matrix, capability, axis, artifact.as_deref()),
                };
                let mut next = Vec::new();
                for row in &rows {
                    for value in &values {
                        let mut extended = row.clone();
                        extended.push((axis, value.clone()));
                        next.push(extended);
                    }
                }
                rows = next;
            }
            cells.extend(rows.into_iter().map(|values| Cell {
                capability: capability.id.clone(),
                values,
            }));
        }
    }
    cells.sort_by_key(Cell::id);
    cells
}

/// Whether `key` applies to `cell`: `all`, or every dot-separated part is one
/// of the cell's values.
pub fn key_matches(key: &str, cell: &Cell) -> bool {
    key == "all" || key.split('.').all(|part| cell.has(part))
}

/// The union of the lists whose keys apply to `cell`.
pub fn resolve(keyed: &Keyed, cell: &Cell) -> BTreeSet<String> {
    keyed
        .iter()
        .filter(|(key, _)| key_matches(key, cell))
        .flat_map(|(_, entries)| entries.iter().cloned())
        .collect()
}

/// The 12 base product cells, venue × store × family in axis order, each with
/// how many cells fall in it. A cell that names no venue or no store counts
/// for each; cells that name no family are left out.
pub fn rollup(matrix: &Matrix, cells: &[Cell]) -> Vec<(String, String, String, usize)> {
    let mut rows = Vec::new();
    for venue in &matrix.axes.venue {
        for backend in &matrix.axes.backend {
            for family in &matrix.axes.family {
                let count = cells
                    .iter()
                    .filter(|cell| cell.value("family") == Some(family.as_str()))
                    .filter(|cell| cell.value("venue").is_none_or(|v| v == venue.as_str()))
                    .filter(|cell| cell.value("backend").is_none_or(|b| b == backend.as_str()))
                    .count();
                rows.push((venue.clone(), backend.clone(), family.clone(), count));
            }
        }
    }
    rows
}

/// One line of the ledger after its `revision N` header.
#[derive(Debug, PartialEq, Eq)]
pub enum Entry {
    /// A required cell.
    Active(String),
    /// A cell a reduction removed, naming that scope change.
    Removed { id: String, change: String },
    /// A cell split or renamed at a revision, naming the cells that carry it on.
    Replaced {
        id: String,
        by: Vec<String>,
        revision: u32,
    },
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Self::Active(id) | Self::Removed { id, .. } | Self::Replaced { id, .. } => id,
        }
    }
}

#[derive(Debug)]
pub struct Ledger {
    pub revision: u32,
    pub entries: Vec<Entry>,
}

pub fn parse_ledger(text: &str) -> Result<Ledger, String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("cells.txt is empty")?;
    let revision = header
        .strip_prefix("revision ")
        .and_then(|number| number.parse().ok())
        .ok_or_else(|| format!("cells.txt must start with `revision N`, not {header:?}"))?;
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        let words: Vec<&str> = line.split(' ').collect();
        let entry = match words.as_slice() {
            [id] if !id.is_empty() => Entry::Active((*id).to_string()),
            [id, "removed", change] => Entry::Removed {
                id: (*id).to_string(),
                change: (*change).to_string(),
            },
            [id, "replaced-by", by, at] => Entry::Replaced {
                id: (*id).to_string(),
                by: by.split(',').map(str::to_string).collect(),
                revision: at
                    .strip_prefix('r')
                    .and_then(|number| number.parse().ok())
                    .ok_or_else(|| format!("cells.txt line {}: {at:?} is not rN", index + 2))?,
            },
            _ => {
                return Err(format!(
                    "cells.txt line {}: cannot read {line:?}",
                    index + 2
                ));
            }
        };
        entries.push(entry);
    }
    Ok(Ledger { revision, entries })
}

pub fn render_ledger(ledger: &Ledger) -> String {
    let mut text = format!("revision {}\n", ledger.revision);
    for entry in &ledger.entries {
        match entry {
            Entry::Active(id) => text.push_str(id),
            Entry::Removed { id, change } => text.push_str(&format!("{id} removed {change}")),
            Entry::Replaced { id, by, revision } => {
                text.push_str(&format!("{id} replaced-by {} r{revision}", by.join(",")))
            }
        }
        text.push('\n');
    }
    text
}

/// Whether a reduction named `change` removes cell `id`, by listing the cell
/// or its capability.
fn removed_by(matrix: &Matrix, change: &str, id: &str) -> bool {
    let capability = id.split('/').next().unwrap_or_default();
    matrix.scope_changes.iter().any(|scope| {
        scope.id == change
            && scope.kind == ScopeKind::Reduction
            && scope
                .removes
                .iter()
                .any(|removed| removed == id || removed == capability)
    })
}

/// What is wrong with `ledger`, given the cells the matrix generates.
pub fn ledger_problems(
    matrix: &Matrix,
    generated: &BTreeSet<String>,
    ledger: &Ledger,
) -> Vec<String> {
    let mut problems = Vec::new();
    if ledger.revision != matrix.revision {
        problems.push(format!(
            "cells.txt is at revision {} but matrix.toml at {}",
            ledger.revision, matrix.revision
        ));
    }
    let mut seen = BTreeSet::new();
    let mut active = BTreeSet::new();
    let replaced: BTreeSet<&str> = ledger
        .entries
        .iter()
        .filter(|entry| matches!(entry, Entry::Replaced { .. }))
        .map(Entry::id)
        .collect();
    for entry in &ledger.entries {
        if !seen.insert(entry.id().to_string()) {
            problems.push(format!(
                "{} appears more than once in cells.txt",
                entry.id()
            ));
        }
        match entry {
            Entry::Active(id) => {
                active.insert(id.clone());
            }
            Entry::Removed { id, change } => {
                if !removed_by(matrix, change, id) {
                    problems.push(format!(
                        "{id} is removed by {change}, which is no reduction that lists it"
                    ));
                }
            }
            Entry::Replaced { id, by, revision } => {
                if *revision == 0 || *revision > matrix.revision {
                    problems.push(format!(
                        "{id} is replaced at revision {revision}, which does not exist yet"
                    ));
                }
                for target in by {
                    if !generated.contains(target) && !replaced.contains(target.as_str()) {
                        problems.push(format!("{id} is replaced by {target}, which is not a cell"));
                    }
                }
            }
        }
    }
    for id in generated.difference(&active) {
        problems.push(format!("{id} is required but not in cells.txt"));
    }
    for id in active.difference(generated) {
        problems.push(format!(
            "{id} is in cells.txt but no longer generated; removing it needs a reduction scope change"
        ));
    }
    problems
}

/// The ledger after this change: new cells are appended, a cell no longer
/// generated becomes a tombstone if a reduction removes it, and a tombstoned
/// cell generated again comes back. Fails when a cell would disappear without
/// a reduction, or when the cells change without a new revision.
pub fn updated_ledger(
    matrix: &Matrix,
    generated: &BTreeSet<String>,
    previous: Option<Ledger>,
) -> Result<Ledger, Vec<String>> {
    let Some(previous) = previous else {
        return Ok(Ledger {
            revision: matrix.revision,
            entries: generated.iter().cloned().map(Entry::Active).collect(),
        });
    };
    let mut problems = Vec::new();
    let mut changed = false;
    let mut known = BTreeSet::new();
    let mut entries = Vec::new();
    for entry in previous.entries {
        known.insert(entry.id().to_string());
        match entry {
            Entry::Active(id) if !generated.contains(&id) => {
                let reduction = matrix
                    .scope_changes
                    .iter()
                    .find(|scope| removed_by(matrix, &scope.id, &id));
                match reduction {
                    Some(scope) => {
                        entries.push(Entry::Removed {
                            id,
                            change: scope.id.clone(),
                        });
                        changed = true;
                    }
                    None => {
                        problems.push(format!(
                            "{id} would leave cells.txt; reducing coverage needs a reduction scope change that removes it"
                        ));
                        entries.push(Entry::Active(id));
                    }
                }
            }
            Entry::Removed { id, .. } if generated.contains(&id) => {
                entries.push(Entry::Active(id));
                changed = true;
            }
            other => entries.push(other),
        }
    }
    for id in generated.difference(&known) {
        entries.push(Entry::Active(id.clone()));
        changed = true;
    }
    if changed && matrix.revision <= previous.revision {
        problems.push(format!(
            "the cells changed, so matrix.toml's revision must exceed {}",
            previous.revision
        ));
    }
    if problems.is_empty() {
        Ok(Ledger {
            revision: matrix.revision,
            entries,
        })
    } else {
        Err(problems)
    }
}

fn generated_ids(matrix: &Matrix) -> BTreeSet<String> {
    expand(matrix).iter().map(Cell::id).collect()
}

#[test]
fn the_cell_ledger_lists_every_required_cell() {
    let matrix = model::load_matrix();
    let generated = generated_ids(&matrix);
    if snapshot::updating() {
        let path = model::path(LEDGER);
        let previous = if path.exists() {
            Some(parse_ledger(&model::read(LEDGER)).unwrap_or_else(|err| panic!("{err}")))
        } else {
            None
        };
        let ledger = updated_ledger(&matrix, &generated, previous).unwrap_or_else(|problems| {
            panic!("cells.txt was not updated:\n{}", problems.join("\n"))
        });
        std::fs::write(&path, render_ledger(&ledger)).expect("write cells.txt");
    }
    let ledger = parse_ledger(&model::read(LEDGER)).unwrap_or_else(|err| panic!("{err}"));
    let problems = ledger_problems(&matrix, &generated, &ledger);
    assert!(
        problems.is_empty(),
        "cells.txt disagrees with matrix.toml (refresh with `UPDATE_SNAPSHOTS=1 cargo test --test parity`):\n{}",
        problems.join("\n")
    );
}

#[test]
fn cell_ids_are_unique() {
    let matrix = model::load_matrix();
    let cells = expand(&matrix);
    assert_eq!(
        generated_ids(&matrix).len(),
        cells.len(),
        "two cells share an ID"
    );
}

#[test]
fn a_cell_deleted_from_the_ledger_by_hand_is_reported() {
    let matrix = model::load_matrix();
    let generated = generated_ids(&matrix);
    let mut entries: Vec<Entry> = generated.iter().cloned().map(Entry::Active).collect();
    let deleted = entries.remove(0);
    let ledger = Ledger {
        revision: matrix.revision,
        entries,
    };
    let problems = ledger_problems(&matrix, &generated, &ledger);
    assert_eq!(
        problems,
        vec![format!("{} is required but not in cells.txt", deleted.id())]
    );
}

#[test]
fn removing_a_cell_needs_a_reduction_that_names_it_and_a_new_revision() {
    let mut matrix = model::load_matrix();
    let generated = generated_ids(&matrix);
    let gone = "spot.order.market/binance/spot";
    let fewer: BTreeSet<String> = generated.iter().filter(|id| *id != gone).cloned().collect();
    let base = matrix.revision;
    let previous = || Ledger {
        revision: base,
        entries: generated.iter().cloned().map(Entry::Active).collect(),
    };

    let refused = updated_ledger(&matrix, &fewer, Some(previous()))
        .expect_err("an unrecorded removal is refused");
    assert!(refused[0].starts_with(gone), "{refused:?}");
    let problems = ledger_problems(&matrix, &fewer, &previous());
    assert!(
        problems.iter().any(|problem| problem.starts_with(gone)),
        "{problems:?}"
    );

    let reduction: model::ScopeChange = toml::from_str(&format!(
        "id = \"SC-9\"\nkind = \"reduction\"\ndate = \"2026-10-22\"\ndecided_by = \"Marko\"\nsummary = \"s\"\nreference = \"r\"\nremoves = [\"{gone}\"]\n"
    ))
    .expect("a scope change parses");
    matrix.scope_changes.push(reduction);
    let stale = updated_ledger(&matrix, &fewer, Some(previous()))
        .expect_err("a reduction without a new revision is refused");
    assert!(stale[0].contains("revision must exceed"), "{stale:?}");

    matrix.revision += 1;
    let ledger = updated_ledger(&matrix, &fewer, Some(previous()))
        .expect("a recorded reduction at a new revision updates the ledger");
    assert!(ledger.entries.contains(&Entry::Removed {
        id: gone.to_string(),
        change: "SC-9".to_string()
    }));
    assert!(ledger_problems(&matrix, &fewer, &ledger).is_empty());
    let text = render_ledger(&ledger);
    assert!(text.contains(&format!("{gone} removed SC-9\n")), "{text}");
    let reread = parse_ledger(&text).expect("the ledger reads back");
    assert_eq!(reread.entries, ledger.entries);
}

#[test]
fn owner_keys_apply_by_every_part_of_the_key() {
    let cell = Cell {
        capability: "c".to_string(),
        values: vec![
            ("venue", "binance".to_string()),
            ("family", "linear".to_string()),
        ],
    };
    assert!(key_matches("all", &cell));
    assert!(key_matches("binance", &cell));
    assert!(key_matches("binance.linear", &cell));
    assert!(!key_matches("binance.inverse", &cell));
    assert!(!key_matches("bybit", &cell));
}
