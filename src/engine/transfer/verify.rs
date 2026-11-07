//! Comparing a copy with its source row by row: both stores read side by
//! side in key order, each side's rows taken into a digest of its own, and
//! every row one side lacks, or holds otherwise, counted as a difference.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fmt;

use sha2::{Digest as _, Sha256};

use crate::model::transfer::write_canonical_row;
use crate::model::{ColumnKind, TransferMarket, TransferRow, TransferTable, TransferValue};
use crate::ports::{StorageError, TransferSource};

/// The most differences a report keeps; it counts every one.
pub const DIFFERENCES_KEPT: usize = 20;

/// What one side read of a table, or of a market of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Digest {
    pub rows: u64,
    /// The first and last key read, when the key is an integer.
    pub first: Option<TransferValue>,
    pub last: Option<TransferValue>,
    /// `sha256:` and the hex of the SHA-256 of the canonical text.
    pub checksum: String,
}

/// Both sides' digests of a table, or of one market of a market table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeReport {
    pub table: &'static str,
    pub market: Option<TransferMarket>,
    pub key: &'static str,
    pub source: Digest,
    pub target: Digest,
}

impl ScopeReport {
    pub fn matches(&self) -> bool {
        self.source == self.target
    }
}

/// A row one side lacks, or holds otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub table: &'static str,
    pub market: Option<TransferMarket>,
    pub key: &'static str,
    pub key_value: TransferValue,
    pub kind: DifferenceKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DifferenceKind {
    OnlyInSource,
    OnlyInTarget,
    /// The row is on both sides, and these columns differ: the first one's
    /// values are given.
    Values {
        columns: Vec<&'static str>,
        source: TransferValue,
        target: TransferValue,
    },
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.table)?;
        if let Some(market) = &self.market {
            write!(f, " {market}")?;
        }
        write!(f, " {}={}: ", self.key, self.key_value)?;
        match &self.kind {
            DifferenceKind::OnlyInSource => f.write_str("only in the source"),
            DifferenceKind::OnlyInTarget => f.write_str("only in the target"),
            DifferenceKind::Values {
                columns,
                source,
                target,
            } => {
                let (first, rest) = columns
                    .split_first()
                    .map_or(("", &[][..]), |(f, r)| (*f, r));
                let (at, source, target) = shown(source, target);
                write!(f, "{first} differs")?;
                if let Some(at) = at {
                    write!(f, " at byte {at}")?;
                }
                write!(f, ": source {source}, target {target}")?;
                if !rest.is_empty() {
                    write!(f, " (and {})", rest.join(", "))?;
                }
                Ok(())
            }
        }
    }
}

/// Texts up to this many characters are shown whole.
const SHOWN_WHOLE: usize = 48;

/// Characters shown on either side of where two long texts first differ.
const SHOWN_AROUND: usize = 16;

/// Two differing values as a report shows them: short ones whole, long
/// texts as the stretch around where they first differ, blobs by their
/// length, with that byte's offset in the source when not shown whole.
fn shown(source: &TransferValue, target: &TransferValue) -> (Option<usize>, String, String) {
    match (source, target) {
        (TransferValue::Text(left), TransferValue::Text(right))
            if left.chars().count().max(right.chars().count()) > SHOWN_WHOLE =>
        {
            let shorter = (
                left.len().min(right.len()),
                left.chars().count().min(right.chars().count()),
            );
            let (at, chars) = left
                .char_indices()
                .zip(right.chars())
                .enumerate()
                .find(|(_, ((_, a), b))| a != b)
                .map_or(shorter, |(index, ((byte, _), _))| (byte, index));
            (Some(at), excerpt(left, chars), excerpt(right, chars))
        }
        (TransferValue::Blob(left), TransferValue::Blob(right)) => {
            let at = left
                .iter()
                .zip(right)
                .position(|(a, b)| a != b)
                .unwrap_or(left.len().min(right.len()));
            (Some(at), source.to_string(), target.to_string())
        }
        _ => (None, source.to_string(), target.to_string()),
    }
}

/// The characters of `text` around the `at`th, quoted, with an ellipsis for
/// what is left out.
fn excerpt(text: &str, at: usize) -> String {
    let start = at.saturating_sub(SHOWN_AROUND);
    let chars: String = text.chars().skip(start).take(2 * SHOWN_AROUND).collect();
    let before = if start > 0 { "…" } else { "" };
    let after = if start + 2 * SHOWN_AROUND < text.chars().count() {
        "…"
    } else {
        ""
    };
    format!("{before}{chars:?}{after}")
}

/// Everything a verification found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    /// Every table, and every market of a market table, in the order read.
    pub scopes: Vec<ScopeReport>,
    /// The first differences, in the order found.
    pub differences: Vec<Difference>,
    /// Every difference, kept or not.
    pub difference_count: u64,
}

impl VerifyReport {
    /// Whether the target is a copy of the source, row for row.
    pub fn is_ok(&self) -> bool {
        self.difference_count == 0 && self.scopes.iter().all(ScopeReport::matches)
    }

    fn add(&mut self, difference: Difference) {
        self.difference_count += 1;
        if self.differences.len() < DIFFERENCES_KEPT {
            self.differences.push(difference);
        }
    }
}

/// Compares `table` on both sides into `report`.
pub(super) fn table<A, B>(
    source: &A,
    target: &B,
    table: &TransferTable,
    batch_rows: usize,
    report: &mut VerifyReport,
) -> Result<(), StorageError>
where
    A: TransferSource + ?Sized,
    B: TransferSource + ?Sized,
{
    let limit = table.batch_rows(batch_rows);
    let key = table.key_index();
    let markets = markets(source, target, table)?;
    for market in markets {
        let mut ours = Side::new(source, table, market.as_ref(), limit);
        let mut theirs = Side::new(target, table, market.as_ref(), limit);
        loop {
            let order = match (ours.peek()?, theirs.peek()?) {
                (None, None) => break,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(left), Some(right)) => left.get(key).cmp(&right.get(key)),
            };
            let difference = |key_value: TransferValue, kind| Difference {
                table: table.name,
                market: market.clone(),
                key: table.key,
                key_value,
                kind,
            };
            match order {
                Ordering::Less => {
                    let row = ours.take();
                    report.add(difference(
                        value_at(&row, key),
                        DifferenceKind::OnlyInSource,
                    ));
                }
                Ordering::Greater => {
                    let row = theirs.take();
                    report.add(difference(
                        value_at(&row, key),
                        DifferenceKind::OnlyInTarget,
                    ));
                }
                Ordering::Equal => {
                    let (left, right) = (ours.take(), theirs.take());
                    if let Some(kind) = values_differ(table, &left, &right) {
                        report.add(difference(value_at(&left, key), kind));
                    }
                }
            }
        }
        let (source, target) = (ours.digest(), theirs.digest());
        report.scopes.push(ScopeReport {
            table: table.name,
            market,
            key: table.key,
            source,
            target,
        });
    }
    Ok(())
}

/// The scopes to compare: each market either side holds, for a market
/// table.
fn markets<A, B>(
    source: &A,
    target: &B,
    table: &TransferTable,
) -> Result<Vec<Option<TransferMarket>>, StorageError>
where
    A: TransferSource + ?Sized,
    B: TransferSource + ?Sized,
{
    if !table.by_market {
        return Ok(vec![None]);
    }
    let mut markets = source.markets(table)?;
    markets.extend(target.markets(table)?);
    markets.sort();
    markets.dedup();
    Ok(markets.into_iter().map(Some).collect())
}

fn value_at(row: &TransferRow, index: usize) -> TransferValue {
    row.get(index).cloned().unwrap_or(TransferValue::Null)
}

/// The columns two rows of one key differ in, if any.
fn values_differ(
    table: &TransferTable,
    left: &TransferRow,
    right: &TransferRow,
) -> Option<DifferenceKind> {
    let differing: Vec<usize> = (0..table.columns.len().max(left.len()).max(right.len()))
        .filter(|&index| left.get(index) != right.get(index))
        .collect();
    let first = *differing.first()?;
    Some(DifferenceKind::Values {
        columns: differing
            .iter()
            .map(|&index| table.columns.get(index).map_or("?", |column| column.name))
            .collect(),
        source: value_at(left, first),
        target: value_at(right, first),
    })
}

/// One store's rows of a scope, read a batch at a time, and the digest of
/// those taken so far.
struct Side<'a, S: TransferSource + ?Sized> {
    store: &'a S,
    table: &'a TransferTable,
    market: Option<&'a TransferMarket>,
    limit: usize,
    buffer: VecDeque<TransferRow>,
    after: Option<TransferValue>,
    exhausted: bool,
    hasher: Sha256,
    rows: u64,
    first: Option<TransferValue>,
    last: Option<TransferValue>,
}

impl<'a, S: TransferSource + ?Sized> Side<'a, S> {
    fn new(
        store: &'a S,
        table: &'a TransferTable,
        market: Option<&'a TransferMarket>,
        limit: usize,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(table.canonical_header());
        Self {
            store,
            table,
            market,
            limit,
            buffer: VecDeque::new(),
            after: None,
            exhausted: false,
            hasher,
            rows: 0,
            first: None,
            last: None,
        }
    }

    /// The next row, read from the store when none is left over.
    fn peek(&mut self) -> Result<Option<&TransferRow>, StorageError> {
        if self.buffer.is_empty() && !self.exhausted {
            let rows =
                self.store
                    .read_rows(self.table, self.market, self.after.as_ref(), self.limit)?;
            let next = rows
                .last()
                .and_then(|row| row.get(self.table.key_index()).cloned());
            // A short batch is the last, and so is one whose key cannot be
            // read past.
            self.exhausted = rows.len() < self.limit || next.is_none();
            self.after = next;
            self.buffer.extend(rows);
        }
        Ok(self.buffer.front())
    }

    /// Takes the row `peek` showed into the digest.
    fn take(&mut self) -> TransferRow {
        let row = self.buffer.pop_front().unwrap_or_default();
        let mut text = Vec::new();
        write_canonical_row(&row, &mut text);
        self.hasher.update(&text);
        self.rows += 1;
        if self.table.key_kind() == ColumnKind::Integer {
            let key = value_at(&row, self.table.key_index());
            self.first.get_or_insert_with(|| key.clone());
            self.last = Some(key);
        }
        row
    }

    fn digest(self) -> Digest {
        Digest {
            rows: self.rows,
            first: self.first,
            last: self.last,
            checksum: format!("sha256:{}", hex::encode(self.hasher.finalize())),
        }
    }
}
