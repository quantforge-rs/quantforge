//! The manifest, `quantforge.toml`: what a bundle runs, and what it needs to
//! run.

use std::collections::{BTreeMap, BTreeSet};

use super::super::ProtocolError;
use super::entry::Entry;
use super::package::{PackageName, PackageVersion};
use super::params::{self, Params, shown_key};
use super::path::BundlePath;
use super::problems::{BundleProblem, ProblemCode};
use super::requirement::{EngineRequirement, PythonVersion};
use super::{BUNDLE_FORMAT, MANIFEST_FILE, MAX_MANIFEST_BYTES};

/// The deepest a manifest's values may nest: the root table is one level,
/// and each table or array inside a value adds one.
pub const MAX_MANIFEST_DEPTH: usize = 32;

/// The keys of a format 1 manifest, in the order the spec lists them.
pub const MANIFEST_KEYS: [&str; 6] = ["format", "entry", "engine", "python", "params", "deps"];

/// A checked manifest: the bundle format it follows, the strategy it runs,
/// the engine and Python versions it runs on, the strategy's parameters,
/// and the Python packages it imports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    format: u32,
    entry: Entry,
    engine: EngineRequirement,
    python: PythonVersion,
    params: Params,
    deps: BTreeMap<PackageName, PackageVersion>,
}

impl Manifest {
    /// The bundle format, [`BUNDLE_FORMAT`].
    pub fn format(&self) -> u32 {
        self.format
    }

    /// The strategy the worker runs.
    pub fn entry(&self) -> &Entry {
        &self.entry
    }

    /// The engine versions the bundle runs on.
    pub fn engine(&self) -> &EngineRequirement {
        &self.engine
    }

    /// The Python version the bundle was written for.
    pub fn python(&self) -> PythonVersion {
        self.python
    }

    /// The strategy's parameters.
    pub fn params(&self) -> &Params {
        &self.params
    }

    /// The packages the strategy imports, with the version of each.
    pub fn deps(&self) -> &BTreeMap<PackageName, PackageVersion> {
        &self.deps
    }
}

/// Checks a manifest in the spec's order: the document, then its format,
/// then each key on its own. `paths` are the bundle's paths, which the entry
/// must name one of.
pub(super) fn parse(
    bytes: &[u8],
    paths: &BTreeSet<BundlePath>,
) -> Result<Manifest, Vec<BundleProblem>> {
    let table = document(bytes).map_err(|problem| vec![problem])?;
    let format = format(&table).map_err(|problem| vec![problem])?;
    let mut problems = Vec::new();
    let entry = keep(entry(&table, paths), &mut problems);
    let engine = keep(engine(&table), &mut problems);
    let python = keep(python(&table), &mut problems);
    let params = match table.get("params") {
        None => Params::default(),
        Some(toml::Value::Table(table)) => {
            let (params, found) = params::from_toml(table);
            problems.extend(found);
            params
        }
        Some(other) => {
            problems.push(BundleProblem::new(
                ProblemCode::ParamNotJson,
                format!(
                    "{MANIFEST_FILE}'s params must be a table, as in [params], not a {}",
                    other.type_str()
                ),
            ));
            Params::default()
        }
    };
    let deps = deps(&table, &mut problems);
    for key in table
        .keys()
        .filter(|key| !MANIFEST_KEYS.contains(&key.as_str()))
    {
        problems.push(invalid(format!(
            "{MANIFEST_FILE} has the key {}, which bundle format {BUNDLE_FORMAT} does not \
             define; its keys are format, entry, engine, python, params, and deps",
            shown_key(key)
        )));
    }
    match (entry, engine, python) {
        (Some(entry), Some(engine), Some(python)) if problems.is_empty() => Ok(Manifest {
            format,
            entry,
            engine,
            python,
            params,
            deps,
        }),
        _ => Err(problems),
    }
}

fn keep<T>(checked: Result<T, BundleProblem>, problems: &mut Vec<BundleProblem>) -> Option<T> {
    checked.map_err(|problem| problems.push(problem)).ok()
}

fn invalid(message: String) -> BundleProblem {
    BundleProblem::new(ProblemCode::ManifestInvalid, message)
}

/// The manifest as a TOML table, if it is bundle TOML: at most
/// `MAX_MANIFEST_BYTES`, UTF-8 without a byte-order mark, TOML 1.0, nested at
/// most `MAX_MANIFEST_DEPTH` levels, and with finite numbers only.
fn document(bytes: &[u8]) -> Result<toml::Table, BundleProblem> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MANIFEST_BYTES {
        return Err(invalid(format!(
            "{MANIFEST_FILE} is {} bytes, over the {MAX_MANIFEST_BYTES} a manifest may hold",
            bytes.len()
        )));
    }
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        return Err(invalid(format!(
            "{MANIFEST_FILE} starts with a byte-order mark, which Python's tomllib refuses; save \
             it as UTF-8 without one"
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| invalid(format!("{MANIFEST_FILE} is not UTF-8: {error}")))?;
    let table: toml::Table = text.parse().map_err(|error: toml::de::Error| {
        let message = error.message().trim().replace('\n', "; ");
        let place = error.span().map(|span| location(text, span.start));
        invalid(match place {
            Some((line, column)) => {
                format!("{MANIFEST_FILE} line {line}, column {column}: {message}")
            }
            None => format!("{MANIFEST_FILE}: {message}"),
        })
    })?;
    let values = table
        .iter()
        .map(|(key, value)| (shown_key(key), value))
        .collect();
    subset_fault(values, 1).map_or(Ok(table), |reason| {
        Err(invalid(format!("{MANIFEST_FILE} {reason}")))
    })
}

/// The 1-based line and column, in characters, of a byte offset.
fn location(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    (line, column)
}

/// The first value, depth first in key order, that bundle TOML refuses: one
/// nested deeper than `MAX_MANIFEST_DEPTH` levels, or a number that is not finite.
fn subset_fault(values: Vec<(String, &toml::Value)>, depth: usize) -> Option<String> {
    if depth > MAX_MANIFEST_DEPTH {
        return Some(format!(
            "nests its values more than {MAX_MANIFEST_DEPTH} levels deep"
        ));
    }
    values.into_iter().find_map(|(at, value)| match value {
        toml::Value::Float(float) if !float.is_finite() => Some(format!(
            "holds {float} at {at}, but a manifest's numbers are finite"
        )),
        toml::Value::Array(items) => subset_fault(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| (format!("{at}[{index}]"), item))
                .collect(),
            depth + 1,
        ),
        toml::Value::Table(table) => subset_fault(
            table
                .iter()
                .map(|(key, item)| (format!("{at}.{}", shown_key(key)), item))
                .collect(),
            depth + 1,
        ),
        _ => None,
    })
}

fn format(table: &toml::Table) -> Result<u32, BundleProblem> {
    match table.get("format") {
        None => Err(invalid(format!(
            "{MANIFEST_FILE} has no format; add format = {BUNDLE_FORMAT} as its first line"
        ))),
        Some(toml::Value::Integer(format)) if *format == i64::from(BUNDLE_FORMAT) => {
            Ok(BUNDLE_FORMAT)
        }
        Some(toml::Value::Integer(format)) => Err(BundleProblem::new(
            ProblemCode::FormatUnsupported,
            format!(
                "{MANIFEST_FILE} follows bundle format {format}, but this side reads format \
                 {BUNDLE_FORMAT} only"
            ),
        )),
        Some(other) => Err(invalid(format!(
            "{MANIFEST_FILE}'s format must be the integer {BUNDLE_FORMAT}, not a {}",
            other.type_str()
        ))),
    }
}

/// The string under `key`, or the problem `code` with the hint `example`.
fn string<'a>(
    table: &'a toml::Table,
    key: &str,
    code: ProblemCode,
    example: &str,
) -> Result<&'a str, BundleProblem> {
    match table.get(key) {
        Some(toml::Value::String(text)) => Ok(text),
        None => Err(BundleProblem::new(
            code,
            format!("{MANIFEST_FILE} has no {key}; add {key} = {example}"),
        )),
        Some(other) => Err(BundleProblem::new(
            code,
            format!(
                "{MANIFEST_FILE}'s {key} must be a string, such as {example}, not a {}",
                other.type_str()
            ),
        )),
    }
}

fn entry(table: &toml::Table, paths: &BTreeSet<BundlePath>) -> Result<Entry, BundleProblem> {
    let code = ProblemCode::EntryInvalid;
    let text = string(table, "entry", code, "\"strategy.py:MyStrategy\"")?;
    let entry: Entry = text
        .parse()
        .map_err(|error: ProtocolError| BundleProblem::new(code, error.to_string()))?;
    if paths.contains(entry.file()) {
        return Ok(entry);
    }
    let python: Vec<&str> = paths
        .iter()
        .map(BundlePath::as_str)
        .filter(|path| path.ends_with(".py"))
        .collect();
    let holds = if python.is_empty() {
        "it holds no Python files".to_string()
    } else {
        format!("its Python files are {}", python.join(", "))
    };
    Err(BundleProblem::new(
        ProblemCode::EntryNotFound,
        format!(
            "the entry names {:?}, which the bundle does not hold; {holds}",
            entry.file().as_str()
        ),
    ))
}

fn engine(table: &toml::Table) -> Result<EngineRequirement, BundleProblem> {
    let code = ProblemCode::EngineInvalid;
    let text = string(table, "engine", code, "\">=0.3.0, <0.4.0\"")?;
    text.parse()
        .map_err(|error: ProtocolError| BundleProblem::new(code, error.to_string()))
}

fn python(table: &toml::Table) -> Result<PythonVersion, BundleProblem> {
    let code = ProblemCode::PythonInvalid;
    let text = string(table, "python", code, "\"3.12\"")?;
    text.parse()
        .map_err(|error: ProtocolError| BundleProblem::new(code, error.to_string()))
}

fn deps(
    table: &toml::Table,
    problems: &mut Vec<BundleProblem>,
) -> BTreeMap<PackageName, PackageVersion> {
    let problem = |message: String| BundleProblem::new(ProblemCode::DependencyInvalid, message);
    let mut deps = BTreeMap::new();
    let entries = match table.get("deps") {
        None => return deps,
        Some(toml::Value::Table(entries)) => entries,
        Some(other) => {
            problems.push(problem(format!(
                "{MANIFEST_FILE}'s deps must be a table of package versions, as in [deps], not a \
                 {}",
                other.type_str()
            )));
            return deps;
        }
    };
    for (name, version) in entries {
        let at = format!("deps.{}", shown_key(name));
        let name =
            PackageName::new(name.as_str()).map_err(|error| problem(format!("{at}: {error}")));
        let version = match version {
            toml::Value::String(version) => PackageVersion::new(version.as_str())
                .map_err(|error| problem(format!("{at}: {error}"))),
            other => Err(problem(format!(
                "{at} must be a version string, such as \"2.1.3\", not a {}",
                other.type_str()
            ))),
        };
        if let (Some(name), Some(version)) = (keep(name, problems), keep(version, problems)) {
            deps.insert(name, version);
        }
    }
    deps
}
