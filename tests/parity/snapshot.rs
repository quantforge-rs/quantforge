//! Generated files: compared byte for byte, and rewritten when
//! `UPDATE_SNAPSHOTS=1`.

use crate::model;

/// Whether this run rewrites the generated files. Value-based on purpose: a
/// leftover `UPDATE_SNAPSHOTS=0` in the shell must not switch every guard off.
pub fn updating() -> bool {
    matches!(
        std::env::var("UPDATE_SNAPSHOTS").ok().as_deref(),
        Some("1") | Some("true")
    )
}

/// Compares `actual` with the committed file, or writes it when updating.
pub fn assert_generated(relative: &str, actual: &str) {
    let path = model::path(relative);
    if updating() {
        std::fs::write(&path, actual)
            .unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "cannot read {}: {err}; run `UPDATE_SNAPSHOTS=1 cargo test --test parity`",
            path.display()
        )
    });
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(old, new)| old != new)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "{relative} differs from line {}; run `UPDATE_SNAPSHOTS=1 cargo test --test parity`",
            line + 1
        );
    }
}
