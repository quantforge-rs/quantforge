//! What a verification reads and reports.

use super::*;

#[test]
fn a_verification_reads_the_same_digests_whatever_the_batch() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    engine(&source, &target, 5000).copy().expect("copy");
    let reports: Vec<VerifyReport> = [1, 2, 3, 7, 5000]
        .into_iter()
        .map(|batch| engine(&source, &target, batch).verify().expect("verify"))
        .collect();
    for report in &reports {
        assert!(report.is_ok());
        assert_eq!(report.scopes, reports[0].scopes);
    }
    // A market table reports each market apart, in byte order.
    let candles: Vec<String> = reports[0]
        .scopes
        .iter()
        .filter(|scope| scope.table == "candles")
        .map(|scope| scope.market.as_ref().expect("a market").to_string())
        .collect();
    let markets: Vec<String> = MARKETS
        .iter()
        .map(|(exchange, symbol, interval)| format!("{exchange} {symbol} {interval}"))
        .collect();
    assert_eq!(candles, markets);
    let dirty = reports[0]
        .scopes
        .iter()
        .find(|scope| scope.table == "segment_dirty")
        .expect("the marks");
    assert_eq!(
        (
            dirty.source.rows,
            dirty.source.first.clone(),
            dirty.source.last.clone()
        ),
        (7, Some(int(3)), Some(int(21)))
    );
}

#[test]
fn a_verification_names_each_difference_and_digests_each_side_alone() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    engine(&source, &target, 2).copy().expect("copy");
    let close = CANDLES.column_index("close").expect("close");
    let mut changed = row_of(&CANDLES, 2, 1);
    changed[close] = text("close.2");
    target.put(&CANDLES, changed);
    target.remove(&ORDER_EVENTS, &row_of(&ORDER_EVENTS, 4, 0));
    target.put(&SEGMENT_DIRTY, row_of(&SEGMENT_DIRTY, 8, 0));

    let report = engine(&source, &target, 2).verify().expect("verify");
    assert!(!report.is_ok());
    let found: Vec<String> = report.differences.iter().map(ToString::to_string).collect();
    assert_eq!(
        found,
        [
            "candles bybit_linear BTCUSDT 1m open_time_ms=6: close differs: source \"close.20\", \
             target \"close.2\"",
            "order_events seq=12: only in the source",
            "segment_dirty id=24: only in the target",
        ]
    );
    let mismatched: Vec<&str> = report
        .scopes
        .iter()
        .filter(|scope| !scope.matches())
        .map(|scope| scope.table)
        .collect();
    assert_eq!(mismatched, ["candles", "order_events", "segment_dirty"]);
    let events = report
        .scopes
        .iter()
        .find(|scope| scope.table == "order_events")
        .expect("events");
    assert_eq!((events.source.rows, events.target.rows), (7, 6));
}

#[test]
fn a_market_only_one_side_holds_is_reported_and_differences_past_the_kept_are_counted() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    engine(&source, &target, 3).copy().expect("copy");
    let extra = |n: i64| {
        let mut row = row_of(&CANDLES, n, 0);
        row[1] = text("ETHUSDT");
        row
    };
    for n in 0..30 {
        target.put(&CANDLES, extra(n));
    }

    let report = engine(&source, &target, 3).verify().expect("verify");
    assert_eq!(report.difference_count, 30);
    assert_eq!(report.differences.len(), DIFFERENCES_KEPT);
    let eth = report
        .scopes
        .iter()
        .find(|scope| scope.market.as_ref().is_some_and(|m| m.symbol == "ETHUSDT"))
        .expect("the market only the target holds");
    assert_eq!((eth.source.rows, eth.target.rows), (0, 30));
    assert_eq!(eth.source.checksum.len(), "sha256:".len() + 64);
}

#[test]
fn a_long_difference_is_shown_around_where_it_starts() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    engine(&source, &target, 7).copy().expect("copy");
    let state = BOT_RUNS.column_index("state_json").expect("state");
    let long = |middle: &str| format!("{}{middle}{}", "a".repeat(40), "z".repeat(40));
    for (store, middle) in [(&source, "SOURCE"), (&target, "TARGET")] {
        let mut row = row_of(&BOT_RUNS, 3, 0);
        row[state] = text(&long(middle));
        store.put(&BOT_RUNS, row);
    }

    let report = engine(&source, &target, 7).verify().expect("verify");
    assert_eq!(
        report.differences[0].to_string(),
        "bot_runs run_id=\"key-003\": state_json differs at byte 40: source \
         …\"aaaaaaaaaaaaaaaaSOURCEzzzzzzzzzz\"…, target …\"aaaaaaaaaaaaaaaaTARGETzzzzzzzzzz\"…"
    );
}
