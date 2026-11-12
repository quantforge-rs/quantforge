//! Frames as bytes: a length and its JSON, read back as written, and the
//! exact JSON of what a worker says of its job.

use super::*;
use crate::protocol::ErrorCode;

fn framed<T: Serialize>(frame: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    write(&mut bytes, frame).expect("a frame");
    bytes
}

#[test]
fn a_frame_is_its_length_then_its_json() {
    let bytes = framed(&ToWorker::Candles { bars: Vec::new() });
    let json = br#"{"type":"candles","bars":[]}"#;
    assert_eq!(
        &bytes[..4],
        &u32::try_from(json.len()).expect("a length").to_be_bytes()
    );
    assert_eq!(&bytes[4..], json);
}

#[test]
fn frames_read_back_as_written_and_the_input_ends_cleanly_between_them() {
    let mut bytes = framed(&FromWorker::Finished {
        outcome: Finished::Failed {
            error: ErrorBody {
                code: ErrorCode::InvalidRequest,
                message: "the job failed".to_string(),
                supported: None,
            },
        },
    });
    bytes.extend(framed(&FromWorker::Finished {
        outcome: Finished::Succeeded {
            results: vec![ResultText {
                name: "report".to_string(),
                media_type: "application/json".to_string(),
                text: "{}".to_string(),
            }],
        },
    }));
    let mut input = bytes.as_slice();
    let first: FromWorker = read(&mut input, 1 << 10).expect("a read").expect("a frame");
    assert!(matches!(
        first,
        FromWorker::Finished { outcome: Finished::Failed { error } }
            if error.message == "the job failed"
    ));
    let second: FromWorker = read(&mut input, 1 << 10).expect("a read").expect("a frame");
    assert!(matches!(
        second,
        FromWorker::Finished { outcome: Finished::Succeeded { results } }
            if results.len() == 1 && results[0].text == "{}"
    ));
    let end: Option<FromWorker> = read(&mut input, 1 << 10).expect("a clean end");
    assert!(end.is_none());
}

#[test]
fn a_frame_longer_than_allowed_is_refused_before_it_is_read() {
    let bytes = framed(&ToWorker::Candles { bars: Vec::new() });
    let error = read::<_, ToWorker>(&mut bytes.as_slice(), 8).expect_err("too long");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        error.to_string(),
        "a frame of 28 bytes, more than the 8 one may hold"
    );
}

#[test]
fn a_frame_cut_short_or_not_json_is_an_error() {
    let bytes = framed(&ToWorker::Candles { bars: Vec::new() });
    for cut in [2, 4, bytes.len() - 1] {
        let error =
            read::<_, ToWorker>(&mut &bytes[..cut], 1 << 10).expect_err("a frame cut short");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
    }
    let mut garbage = 3_u32.to_be_bytes().to_vec();
    garbage.extend(b"abc");
    let error = read::<_, ToWorker>(&mut garbage.as_slice(), 1 << 10).expect_err("not json");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn the_build_names_this_release_and_the_frames_version() {
    assert_eq!(
        build(),
        format!("quantforge {} frames 3", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_backtests_rows_and_summary_have_their_exact_json_and_keep_every_digit() {
    let trade = TradeRow {
        symbol: "BTCUSDT".to_string(),
        entry_time_ms: 60_000,
        exit_time_ms: 179_999,
        entry_price: "81940.01000000".parse().expect("a price"),
        exit_price: "82010.5".parse().expect("a price"),
        qty: "0.12203".parse().expect("a quantity"),
        gross_quote_pnl: "-11.40".parse().expect("an amount"),
    };
    let bytes = framed(&FromWorker::Trades {
        rows: vec![TradeFrame::from(&trade)],
    });
    assert_eq!(
        std::str::from_utf8(&bytes[4..]).expect("JSON"),
        r#"{"type":"trades","rows":[{"symbol":"BTCUSDT","entry_time_ms":60000,"exit_time_ms":179999,"entry_price":"81940.01000000","exit_price":"82010.5","qty":"0.12203","gross_quote_pnl":"-11.40"}]}"#
    );
    let frame: FromWorker = read(&mut bytes.as_slice(), 1 << 10)
        .expect("a read")
        .expect("a frame");
    let FromWorker::Trades { rows } = frame else {
        panic!("a trades frame, not {frame:?}");
    };
    let back: Vec<TradeRow> = rows.into_iter().map(TradeRow::from).collect();
    assert_eq!(back, [trade]);
    assert_eq!(back[0].gross_quote_pnl.to_string(), "-11.40");

    let point = EquityRow {
        open_time_ms: 60_000,
        equity: "10262.0".parse().expect("an amount"),
    };
    let bytes = framed(&FromWorker::Equity {
        rows: vec![EquityFrame::from(&point)],
    });
    assert_eq!(
        std::str::from_utf8(&bytes[4..]).expect("JSON"),
        r#"{"type":"equity","rows":[{"open_time_ms":60000,"equity":"10262.0"}]}"#
    );
    let number = br#"{"type":"equity","rows":[{"open_time_ms":60000,"equity":10262.0}]}"#;
    let mut framed_number = u32::try_from(number.len())
        .expect("a length")
        .to_be_bytes()
        .to_vec();
    framed_number.extend(number);
    assert!(
        read::<_, FromWorker>(&mut framed_number.as_slice(), 1 << 10).is_err(),
        "an amount as a JSON number is refused"
    );

    let metrics: Metrics = serde_json::from_str(
        r#"{"initial_cash":"10000","final_equity":"10250.50","total_return_pct":"2.5050","max_drawdown_pct":"0","trade_count":1}"#,
    )
    .expect("metrics");
    let bytes = framed(&FromWorker::Finished {
        outcome: Finished::Backtest { metrics },
    });
    assert_eq!(
        std::str::from_utf8(&bytes[4..]).expect("JSON"),
        r#"{"type":"finished","outcome":{"kind":"backtest","metrics":{"initial_cash":"10000","final_equity":"10250.50","total_return_pct":"2.5050","max_drawdown_pct":"0","trade_count":1}}}"#
    );
}

#[test]
fn a_line_and_progress_have_their_exact_json() {
    let bytes = framed(&FromWorker::Log {
        level: LogLevel::Warning,
        message: "no trade in 500 bars".to_string(),
        dropped_bytes: 0,
    });
    assert_eq!(
        std::str::from_utf8(&bytes[4..]).expect("JSON"),
        r#"{"type":"log","level":"warning","message":"no trade in 500 bars"}"#
    );
    let frame: FromWorker = read(&mut bytes.as_slice(), 1 << 10)
        .expect("a read")
        .expect("a frame");
    assert!(matches!(
        frame,
        FromWorker::Log { level: LogLevel::Warning, ref message, dropped_bytes: 0 }
            if message == "no trade in 500 bars"
    ));

    let cut = framed(&FromWorker::Log {
        level: LogLevel::Info,
        message: "the start".to_string(),
        dropped_bytes: 4096,
    });
    assert_eq!(
        std::str::from_utf8(&cut[4..]).expect("JSON"),
        r#"{"type":"log","level":"info","message":"the start","dropped_bytes":4096}"#
    );

    let bytes = framed(&FromWorker::Progress { bars_done: 720 });
    assert_eq!(
        std::str::from_utf8(&bytes[4..]).expect("JSON"),
        r#"{"type":"progress","bars_done":720}"#
    );
    let frame: FromWorker = read(&mut bytes.as_slice(), 1 << 10)
        .expect("a read")
        .expect("a frame");
    assert!(matches!(frame, FromWorker::Progress { bars_done: 720 }));
}
