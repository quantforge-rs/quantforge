//! The v0.3 capability parity matrix in `docs/parity/`: its structure and
//! vocabularies, the cell ledger that guards its coverage, the evidence ledger,
//! the frozen product universe, the rendered `matrix.md`, and the futures
//! contract's types against the names the matrix freezes.
//!
//! Refresh `cells.txt` and `matrix.md` with
//! `UPDATE_SNAPSHOTS=1 cargo test --test parity`. The cell ledger refuses a
//! removal that no reduction scope change names.

mod cells;
mod contract;
mod evidence;
mod model;
mod render;
mod rules;
mod snapshot;
mod states;
mod universe;
