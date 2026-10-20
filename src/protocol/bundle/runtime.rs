//! What a server runs bundles on, and whether a bundle can run there.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::manifest::Manifest;
use super::package::{PackageName, PackageVersion};
use super::problems::{BundleProblem, ProblemCode};
use super::requirement::{EngineVersion, PythonVersion};

/// What a server's workers run: the engine version, the Python version, and
/// the Python packages installed, with every version of each. The packages
/// are the server's allowlist: a bundle may import those and no others. A
/// server can advertise its runtime, so that a client checks a bundle before
/// it uploads it.
///
/// ```
/// use quantforge::protocol::bundle::Runtime;
///
/// let json = r#"{"engine":"0.3.0","python":"3.12","packages":{"numpy":["2.1.3"],"pandas":["2.2.2","2.2.3"]}}"#;
/// let runtime: Runtime = serde_json::from_str(json)?;
/// assert_eq!(runtime.engine.to_string(), "0.3.0");
/// assert_eq!(serde_json::to_string(&runtime)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runtime {
    /// The engine version the workers run.
    pub engine: EngineVersion,
    /// The Python version the workers run.
    pub python: PythonVersion,
    /// The packages installed, with every version of each.
    pub packages: BTreeMap<PackageName, BTreeSet<PackageVersion>>,
}

/// Every reason `manifest` cannot run on `runtime`: an engine its
/// requirement refuses, another Python version, and each dependency the
/// runtime does not offer at the pinned version.
pub(super) fn admission(manifest: &Manifest, runtime: &Runtime) -> Vec<BundleProblem> {
    let mut problems = Vec::new();
    if !manifest.engine().matches(runtime.engine) {
        problems.push(BundleProblem::new(
            ProblemCode::EngineMismatch,
            format!(
                "the bundle needs engine {}, but this server runs {}; change engine in the \
                 manifest or use a matching server",
                manifest.engine(),
                runtime.engine
            ),
        ));
    }
    if manifest.python() != runtime.python {
        problems.push(BundleProblem::new(
            ProblemCode::PythonMismatch,
            format!(
                "the bundle was written for Python {}, but this server runs Python {}",
                manifest.python(),
                runtime.python
            ),
        ));
    }
    for (name, version) in manifest.deps() {
        match runtime.packages.get(name) {
            None => problems.push(BundleProblem::new(
                ProblemCode::DependencyNotAllowed,
                format!(
                    "the bundle imports {name}, which this server does not offer; {}",
                    offered_packages(&runtime.packages)
                ),
            )),
            Some(versions) if !versions.contains(version) => problems.push(BundleProblem::new(
                ProblemCode::DependencyVersionUnavailable,
                format!(
                    "the bundle pins {name} {version}, but this server offers {name} {} only",
                    join(versions.iter().map(PackageVersion::as_str))
                ),
            )),
            Some(_) => {}
        }
    }
    problems
}

fn offered_packages(packages: &BTreeMap<PackageName, BTreeSet<PackageVersion>>) -> String {
    if packages.is_empty() {
        "it offers no packages".to_string()
    } else {
        format!(
            "it offers {}",
            join(packages.keys().map(PackageName::as_str))
        )
    }
}

fn join<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items.collect::<Vec<_>>().join(", ")
}
