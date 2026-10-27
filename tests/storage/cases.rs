//! The shared identity cases, `tests/fixtures/storage_identity_cases.json`,
//! written by `spikes/storage-contract/identity_cases.py` and never by the
//! Rust code under test.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cases {
    pub format: u32,
    pub note: String,
    pub markets: Vec<MarketCase>,
    pub decimals: Vec<String>,
    pub venue_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketCase {
    pub venue: String,
    pub category: String,
    /// As a user might type it: the store normalizes it.
    pub symbol: String,
    pub interval: String,
    pub storage: StorageSpelling,
    pub wire: WireSpelling,
    pub state_json_exchange: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSpelling {
    pub exchange: String,
    pub symbol: String,
    pub interval: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSpelling {
    pub exchange: String,
    pub category: String,
    pub symbol: String,
    pub interval: String,
}

pub fn fixture(relative: &str) -> std::path::PathBuf {
    relative.split('/').fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        |path, part| path.join(part),
    )
}

pub fn load() -> Cases {
    let path = fixture("tests/fixtures/storage_identity_cases.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let cases: Cases = serde_json::from_str(&text).expect("storage identity cases");
    assert_eq!(cases.format, 1);
    assert!(cases.note.contains("not by the Rust code under test"));
    cases
}
