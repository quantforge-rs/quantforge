//! The parity matrix's files, parsed with serde.
//!
//! Nothing here uses the rest of this test target, so a later gate such as
//! ALG-769 can include the file with `#[path]` and read the same structures.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

/// The schema version `matrix.toml` declares.
pub const MATRIX_FORMAT: u32 = 1;

/// The axes in the order cell IDs join their values.
pub const AXES: [&str; 6] = ["artifact", "surface", "venue", "backend", "family", "side"];

pub const MATRIX: &str = "docs/parity/matrix.toml";
pub const LEDGER: &str = "docs/parity/cells.txt";
pub const EVIDENCE: &str = "docs/parity/evidence.jsonl";
pub const RENDERED: &str = "docs/parity/matrix.md";
pub const REQUIREMENTS: &str = "docs/automation-requirements.md";
pub const VENUES: &str = "docs/futures-venues.md";

/// A path under the crate root.
pub fn path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// A file under the crate root, as text.
pub fn read(relative: &str) -> String {
    let path = path(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

/// The parsed `matrix.toml`.
pub fn load_matrix() -> Matrix {
    toml::from_str(&read(MATRIX)).unwrap_or_else(|err| panic!("{MATRIX} does not parse: {err}"))
}

/// The dated universe list that `matrix.toml` names.
pub fn load_listing(matrix: &Matrix) -> Listing {
    let relative = format!("docs/parity/{}", matrix.universe.list);
    serde_json::from_str(&read(&relative))
        .unwrap_or_else(|err| panic!("{relative} does not parse: {err}"))
}

/// The evidence ledger: one JSON record per non-empty line, or why a line does
/// not parse.
pub fn parse_evidence(text: &str) -> Vec<Result<Record, String>> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|err| format!("line {}: {err}", index + 1))
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matrix {
    pub format: u32,
    pub revision: u32,
    pub source_date: String,
    pub release: String,
    pub consumers: Vec<String>,
    pub axes: Axes,
    pub stories: BTreeMap<String, String>,
    pub units: BTreeMap<String, String>,
    pub errors: Errors,
    pub states: BTreeMap<String, Machine>,
    pub policy: Policy,
    pub universe: Universe,
    #[serde(rename = "area")]
    pub areas: Vec<Area>,
    #[serde(rename = "capability")]
    pub capabilities: Vec<Capability>,
    #[serde(rename = "artifact")]
    pub artifacts: Vec<Artifact>,
    #[serde(rename = "optional_value")]
    pub optional_values: Vec<OptionalValue>,
    #[serde(rename = "blocker")]
    pub blockers: Vec<Blocker>,
    #[serde(rename = "scope_change")]
    pub scope_changes: Vec<ScopeChange>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Axes {
    pub artifact: Vec<String>,
    pub surface: Vec<String>,
    pub venue: Vec<String>,
    pub backend: Vec<String>,
    pub family: Vec<String>,
    pub side: Vec<String>,
}

impl Axes {
    /// The values of `axis`, or `None` for a name that is not an axis.
    pub fn values(&self, axis: &str) -> Option<&[String]> {
        match axis {
            "artifact" => Some(&self.artifact),
            "surface" => Some(&self.surface),
            "venue" => Some(&self.venue),
            "backend" => Some(&self.backend),
            "family" => Some(&self.family),
            "side" => Some(&self.side),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Errors {
    pub exchange: Vec<String>,
    pub storage: Vec<String>,
    pub wire: Vec<String>,
    pub refusal: Vec<String>,
}

impl Errors {
    /// Each vocabulary, by the name areas refer to it with.
    pub fn named(&self) -> [(&'static str, &[String]); 4] {
        [
            ("exchange", &self.exchange),
            ("storage", &self.storage),
            ("wire", &self.wire),
            ("refusal", &self.refusal),
        ]
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    pub initial: String,
    pub states: Vec<String>,
    pub terminal: Vec<String>,
    pub transitions: Vec<String>,
}

impl Machine {
    /// Each transition as `(from, to)`, or `None` where it is not `a -> b`.
    pub fn edges(&self) -> Vec<Option<(&str, &str)>> {
        self.transitions
            .iter()
            .map(|transition| transition.split_once(" -> "))
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub reference: String,
    pub environments: Vec<String>,
    pub profile_identity: Vec<String>,
    pub account_ref: Vec<String>,
    pub position_mode: String,
    pub margin_modes: Vec<String>,
    pub refused_modes: Vec<String>,
    pub changes_account_settings: bool,
    pub rules: Vec<String>,
    pub signals: Signals,
    pub evidence: EvidencePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signals {
    pub reference: String,
    pub age_limit_default_s: u32,
    pub age_limit_min_s: u32,
    pub age_limit_max_s: u32,
    pub future_allowance_s: u32,
    pub drift_limit: String,
    pub propagation_delay_s: u32,
    pub settle_window_s: u32,
    pub repair_attempts: u32,
    pub protective_close_attempts: u32,
    pub attempt_window_s: u32,
    pub unprotected_alert_interval_s: u32,
    pub operator_move_window_s: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePolicy {
    pub reference: String,
    /// Live environments that count, keyed like owners.
    pub environments: BTreeMap<String, Vec<String>>,
    pub postgres_versions: Vec<String>,
    pub artifacts: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Universe {
    pub rule_version: u32,
    pub list: String,
    pub reference: String,
    pub environments: BTreeMap<String, Vec<String>>,
    pub rule: Vec<UniverseRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UniverseRule {
    pub venue: String,
    pub family: String,
    #[serde(rename = "match")]
    pub selects: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    pub id: String,
    pub title: String,
    pub inputs: String,
    pub outputs: String,
    pub states: Vec<String>,
    pub errors: Vec<String>,
    pub units: Vec<String>,
    pub quality: String,
    pub persistence: String,
    pub compare: String,
}

/// Story IDs (or, for flags, capability names) keyed by `all`, by an axis
/// value, or by values joined with dots.
pub type Keyed = BTreeMap<String, Vec<String>>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub id: String,
    pub area: String,
    pub title: String,
    pub axes: Vec<String>,
    #[serde(default)]
    pub only: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub reason: String,
    /// The capability checks a refusal across a domain boundary.
    #[serde(default)]
    pub refusal: bool,
    pub passes: String,
    pub requires: Vec<String>,
    pub revised: u32,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub flags: Keyed,
    pub implementation: Keyed,
    pub evidence: Keyed,
}

impl Capability {
    pub fn spans(&self, axis: &str) -> bool {
        self.axes.iter().any(|spanned| spanned == axis)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub title: String,
    pub surfaces: Vec<String>,
    pub backends: Vec<String>,
    pub published: bool,
    pub platforms: Vec<String>,
    pub requirement: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionalValue {
    pub venue: String,
    pub field: String,
    pub absent_when: String,
    pub representation: String,
    pub source: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocker {
    pub item: u32,
    pub capability: String,
    pub cells: BTreeMap<String, String>,
    pub effect: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeChange {
    pub id: String,
    pub kind: ScopeKind,
    pub date: String,
    pub decided_by: String,
    pub summary: String,
    pub reference: String,
    pub removes: Vec<String>,
}

/// An exclusion records scope that never had cells; a reduction removes cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Exclusion,
    Reduction,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exclusion => "exclusion",
            Self::Reduction => "reduction",
        }
    }
}

/// One line of `evidence.jsonl`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub cell: String,
    pub result: Outcome,
    pub story: String,
    pub matrix_revision: u32,
    /// The full commit of the code under test.
    pub commit: String,
    pub kind: EvidenceKind,
    /// The venue environment of a live run.
    #[serde(default)]
    pub environment: Option<String>,
    /// The store server's version for a real-store run, such as PostgreSQL 16.
    #[serde(default)]
    pub backend_version: Option<String>,
    /// The platform and digest of the published file an artifact run used.
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub fixture: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    pub date: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Only pass and fail exist: a skip is not a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Replay,
    Live,
    RealStore,
    Artifact,
}

impl EvidenceKind {
    pub const ALL: [EvidenceKind; 4] = [Self::Replay, Self::Live, Self::RealStore, Self::Artifact];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Live => "live",
            Self::RealStore => "real_store",
            Self::Artifact => "artifact",
        }
    }
}

/// The dated list of what a universe rule selected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub rule_version: u32,
    pub rule: BTreeMap<String, String>,
    pub fetched_at: String,
    pub sources: Vec<Source>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub venue: String,
    pub category: String,
    pub environment: String,
    pub url: String,
    pub symbols: Vec<String>,
}
