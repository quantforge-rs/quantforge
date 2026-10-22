//! The evidence ledger, `evidence.jsonl`: what every record must say. The
//! committed ledger is checked, and so are records made to fail.

use std::collections::BTreeMap;

use crate::cells::{self, Cell};
use crate::model::{self, EVIDENCE, EvidenceKind, Matrix, Outcome, Record};
use crate::rules::is_date;

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn record_problems(matrix: &Matrix, cell: &Cell, record: &Record) -> Vec<String> {
    let mut problems = Vec::new();
    let capability = matrix
        .capabilities
        .iter()
        .find(|capability| capability.id == cell.capability)
        .expect("every cell has its capability");
    if !cells::resolve(&capability.evidence, cell).contains(&record.story) {
        problems.push(format!(
            "{} does not own evidence for this cell",
            record.story
        ));
    }
    if record.matrix_revision == 0 || record.matrix_revision > matrix.revision {
        problems.push(format!(
            "matrix revision {} does not exist",
            record.matrix_revision
        ));
    }
    if !is_hex(&record.commit, 40) {
        problems.push(format!(
            "commit {:?} is not a full lowercase SHA",
            record.commit
        ));
    }
    if !is_date(&record.date) {
        problems.push(format!("date {:?} is not YYYY-MM-DD", record.date));
    }
    if !capability
        .requires
        .iter()
        .any(|kind| kind == record.kind.as_str())
    {
        problems.push(format!(
            "{} evidence is not what {} requires",
            record.kind.as_str(),
            capability.id
        ));
    }
    let live_environments = cells::resolve(&matrix.policy.evidence.environments, cell);
    match (&record.kind, &record.environment) {
        (EvidenceKind::Live, Some(environment)) if live_environments.contains(environment) => {}
        (EvidenceKind::Live, environment) => problems.push(format!(
            "a live run here needs one of {live_environments:?}, not {environment:?}"
        )),
        (_, Some(environment)) => problems.push(format!(
            "only a live run names an environment, not {environment}"
        )),
        (_, None) => {}
    }
    match (&record.kind, &record.backend_version) {
        (EvidenceKind::RealStore, Some(version)) => {
            let postgres = cell.value("backend") == Some("postgres");
            if version.trim().is_empty()
                || (postgres && !matrix.policy.evidence.postgres_versions.contains(version))
            {
                problems.push(format!(
                    "backend version {version:?} is not one the policy names"
                ));
            }
        }
        (EvidenceKind::RealStore, None) => {
            problems.push("a real-store run names its server version".to_string())
        }
        (_, Some(_)) => problems.push("only a real-store run names a backend version".to_string()),
        (_, None) => {}
    }
    let artifact = cell
        .value("artifact")
        .and_then(|id| matrix.artifacts.iter().find(|artifact| artifact.id == id));
    match (&record.kind, &record.platform, &record.digest) {
        (EvidenceKind::Artifact, Some(platform), Some(digest)) => {
            if artifact.is_some_and(|artifact| !artifact.platforms.contains(platform)) {
                problems.push(format!(
                    "platform {platform} is not one the artifact is published for"
                ));
            }
            if !digest
                .strip_prefix("sha256:")
                .is_some_and(|hex| is_hex(hex, 64))
            {
                problems.push(format!("digest {digest:?} is not sha256:<64 hex>"));
            }
        }
        (EvidenceKind::Artifact, _, _) => {
            problems.push("an artifact run names its platform and digest".to_string())
        }
        (_, None, None) => {}
        _ => problems.push("only an artifact run names a platform or digest".to_string()),
    }
    if record.result == Outcome::Fail && record.note.is_none() {
        problems.push("a failing record says why in its note".to_string());
    }
    for text in [&record.fixture, &record.link, &record.note]
        .into_iter()
        .flatten()
    {
        if text.is_empty() || text.trim() != text {
            problems.push(format!("{text:?} is empty or padded"));
        }
    }
    problems
}

/// What is wrong with the records in `text`, an evidence ledger.
pub fn evidence_problems(matrix: &Matrix, text: &str) -> Vec<String> {
    let cells: BTreeMap<String, Cell> = cells::expand(matrix)
        .into_iter()
        .map(|cell| (cell.id(), cell))
        .collect();
    let mut problems = Vec::new();
    for (index, parsed) in model::parse_evidence(text).into_iter().enumerate() {
        let record = match parsed {
            Ok(record) => record,
            Err(err) => {
                problems.push(err);
                continue;
            }
        };
        let Some(cell) = cells.get(&record.cell) else {
            problems.push(format!(
                "record {}: {} is not a cell",
                index + 1,
                record.cell
            ));
            continue;
        };
        for problem in record_problems(matrix, cell, &record) {
            problems.push(format!("record {} ({}): {problem}", index + 1, record.cell));
        }
    }
    problems
}

#[test]
fn the_evidence_ledger_holds_only_valid_records() {
    let matrix = model::load_matrix();
    let problems = evidence_problems(&matrix, &model::read(EVIDENCE));
    assert!(problems.is_empty(), "{EVIDENCE}:\n{}", problems.join("\n"));
}

#[test]
fn the_evidence_check_refuses_skips_strangers_and_loose_records() {
    let matrix = model::load_matrix();
    let valid = r#"{"cell":"futures.entry/binance/postgres/linear/long","result":"pass","story":"ALG-742","matrix_revision":1,"commit":"0123456789abcdef0123456789abcdef01234567","kind":"live","environment":"demo","date":"2026-10-22"}"#;
    assert_eq!(evidence_problems(&matrix, valid), Vec::<String>::new());
    let artifact = r#"{"cell":"release.artifact/wheel/python/bybit/postgres","result":"fail","story":"ALG-384","matrix_revision":1,"commit":"0123456789abcdef0123456789abcdef01234567","kind":"artifact","platform":"linux","digest":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","date":"2026-10-22","note":"the smoke run found no PostgreSQL"}"#;
    assert_eq!(evidence_problems(&matrix, artifact), Vec::<String>::new());
    let broken = [
        ("a skip", valid.replace(r#""result":"pass""#, r#""result":"skipped""#)),
        ("an unknown cell", valid.replace("futures.entry/", "futures.entries/")),
        ("a consumer as owner", valid.replace("ALG-742", "ALG-731")),
        ("a short commit", valid.replace("0123456789abcdef0123456789abcdef01234567", "0123456")),
        ("no live environment", valid.replace(r#","environment":"demo""#, "")),
        ("a testnet Binance futures run", valid.replace(r#""demo""#, r#""testnet""#)),
        ("a kind the capability does not require", valid.replace(r#""kind":"live","environment":"demo""#, r#""kind":"artifact","platform":"linux","digest":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855""#)),
        ("an unknown field", valid.replace(r#""date""#, r#""waiver":true,"date""#)),
        ("a failure without a note", artifact.replace(r#","note":"the smoke run found no PostgreSQL""#, "")),
        ("an artifact without a digest", artifact.replace(r#","digest":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855""#, "")),
    ];
    for (what, line) in broken {
        assert!(
            !evidence_problems(&matrix, &line).is_empty(),
            "{what} passed: {line}"
        );
    }
}
