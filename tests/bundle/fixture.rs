//! The shared cases in `tests/fixtures/bundle_cases.json`, which every
//! implementation of the spec, the Python SDK's included, must pass. Their
//! code hashes come from an independent Python implementation of the
//! canonical tar, never from the code under test.

use std::collections::BTreeSet;

use quantforge::protocol::bundle::{
    BUNDLE_FORMAT, Bundle, BundleError, BundleLimits, ProblemCode, Runtime,
};
use serde::Deserialize;

const CASES: &str = include_str!("../fixtures/bundle_cases.json");

#[derive(Deserialize)]
struct Fixture {
    format: u32,
    cases: Vec<Case>,
}

#[derive(Clone, Deserialize)]
pub struct Case {
    pub name: String,
    files: Option<Vec<FileSpec>>,
    archive: Option<ArchiveSpec>,
    limits: Option<BundleLimits>,
    runtime: Option<Runtime>,
    pub code_hash: Option<String>,
    problems: Option<Vec<String>>,
    admission: Option<Vec<String>>,
}

#[derive(Clone, Deserialize)]
struct FileSpec {
    path: String,
    text: Option<String>,
    hex: Option<String>,
}

#[derive(Clone, Deserialize)]
struct ArchiveSpec {
    of: String,
    ops: Vec<Op>,
}

/// A change to an archive, applied in order to another case's canonical tar.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Op {
    /// Overwrites bytes at an offset.
    Patch {
        offset: usize,
        hex: String,
    },
    /// Rewrites the checksum of the header in this 512-byte block.
    FixChecksum(usize),
    AppendZeros(usize),
    AppendHex(String),
    /// Drops this many bytes from the end.
    Truncate(usize),
    /// Exchanges two byte ranges of the same length.
    Swap {
        a: usize,
        b: usize,
        length: usize,
    },
}

pub fn cases() -> Vec<Case> {
    let fixture: Fixture = serde_json::from_str(CASES).expect("the fixture parses");
    assert_eq!(fixture.format, BUNDLE_FORMAT, "the fixture's bundle format");
    fixture.cases
}

pub fn case(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("no case {name:?}"))
}

impl Case {
    /// The case's files, as paths and bytes.
    pub fn files(&self) -> Vec<(String, Vec<u8>)> {
        self.files
            .as_ref()
            .unwrap_or_else(|| panic!("{}: a case of files", self.name))
            .iter()
            .map(|file| {
                let bytes = match (&file.text, &file.hex) {
                    (Some(text), None) => text.as_bytes().to_vec(),
                    (None, Some(hex)) => hex::decode(hex).expect("hex contents"),
                    _ => panic!("{}: a file has text or hex", self.name),
                };
                (file.path.clone(), bytes)
            })
            .collect()
    }

    fn limits(&self) -> BundleLimits {
        self.limits.unwrap_or_default()
    }

    fn build(&self, all: &[Case]) -> Result<Bundle, BundleError> {
        match &self.archive {
            None => Bundle::from_files(self.files(), &self.limits()),
            Some(archive) => {
                let base = all
                    .iter()
                    .find(|case| case.name == archive.of)
                    .unwrap_or_else(|| panic!("{}: no case {:?}", self.name, archive.of));
                let mut tar = Bundle::from_files(base.files(), &BundleLimits::default())
                    .expect("the base case is a bundle")
                    .canonical_tar();
                for op in &archive.ops {
                    apply(&mut tar, op);
                }
                Bundle::from_canonical_tar(&tar, &self.limits())
            }
        }
    }
}

fn apply(tar: &mut Vec<u8>, op: &Op) {
    match op {
        Op::Patch { offset, hex } => {
            let bytes = hex::decode(hex).expect("hex patch");
            tar[*offset..offset + bytes.len()].copy_from_slice(&bytes);
        }
        Op::FixChecksum(block) => {
            let header = &mut tar[block * 512..(block + 1) * 512];
            header[148..156].copy_from_slice(b"        ");
            let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
            header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        }
        Op::AppendZeros(count) => tar.resize(tar.len() + count, 0),
        Op::AppendHex(hex) => tar.extend(hex::decode(hex).expect("hex bytes")),
        Op::Truncate(count) => tar.truncate(tar.len() - count),
        Op::Swap { a, b, length } => {
            let first = tar[*a..a + length].to_vec();
            let second = tar[*b..b + length].to_vec();
            tar[*a..a + length].copy_from_slice(&second);
            tar[*b..b + length].copy_from_slice(&first);
        }
    }
}

fn sorted_codes(error: &BundleError) -> Vec<String> {
    let mut codes: Vec<String> = error
        .problems()
        .iter()
        .map(|problem| problem.code().as_str().to_string())
        .collect();
    codes.sort();
    codes
}

#[test]
fn every_shared_case_holds() {
    let all = cases();
    assert!(all.len() >= 60, "found only {} cases", all.len());
    for case in &all {
        let name = &case.name;
        let expectations = [case.code_hash.is_some(), case.problems.is_some()];
        assert_eq!(
            expectations.iter().filter(|&&set| set).count(),
            1,
            "{name}: one expectation"
        );
        let built = case.build(&all);
        if let Some(problems) = &case.problems {
            match built {
                Ok(bundle) => panic!("{name}: accepted as {}", bundle.code_hash()),
                Err(error) => assert_eq!(&sorted_codes(&error), problems, "{name}: {error}"),
            }
            continue;
        }
        let bundle = built.unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            Some(bundle.code_hash().to_string()),
            case.code_hash,
            "{name}: the code hash"
        );
        // An upload of the canonical tar reads back to the same bundle.
        let tar = bundle.canonical_tar();
        let read = Bundle::from_canonical_tar(&tar, &case.limits()).expect("its own canonical tar");
        assert_eq!(read, bundle, "{name}: the round trip");
        if let Some(runtime) = &case.runtime {
            let admission = bundle
                .admit(runtime)
                .err()
                .map_or_else(Vec::new, |error| sorted_codes(&error));
            assert_eq!(
                Some(&admission),
                case.admission.as_ref(),
                "{name}: admission"
            );
        }
    }
}

// Every code but one appears in a shared case; a symlink or socket exists
// only on a filesystem, so `not_a_regular_file` is tested on one.
#[test]
fn the_shared_cases_exercise_every_problem_code() {
    let found: BTreeSet<String> = cases()
        .into_iter()
        .flat_map(|case| case.problems.into_iter().chain(case.admission).flatten())
        .collect();
    for code in ProblemCode::ALL {
        let covered = found.contains(code.as_str());
        assert_eq!(
            covered,
            code != ProblemCode::NotARegularFile,
            "{}",
            code.as_str()
        );
    }
}
