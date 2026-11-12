//! A log's numbering and cursors, what it drops to stay within its limits
//! and how it says so, its progress, its end, and its stored form.

use super::*;

fn limits(max_lines: usize, max_bytes: usize, max_line_bytes: usize) -> LogLimits {
    LogLimits {
        max_lines,
        max_bytes,
        max_line_bytes,
    }
}

/// A log of `count` lines `line 1`, `line 2`, and so on.
fn written(limits: LogLimits, count: u64) -> JobLog {
    let log = JobLog::new(limits);
    for n in 1..=count {
        log.line(LogLevel::Info, &format!("line {n}"));
    }
    log
}

fn numbers(read: &LogRead) -> Vec<u64> {
    read.lines.iter().map(|line| line.seq).collect()
}

#[test]
fn lines_are_numbered_from_one_and_read_back_in_pages_after_a_cursor() {
    let log = written(LogLimits::default(), 5);
    let first = log.read(0, 2);
    assert_eq!(numbers(&first), [1, 2]);
    assert_eq!(first.lines[1].message, "line 2");
    assert_eq!((first.dropped_lines, first.next_after), (0, 2));
    assert!(!first.complete);

    let rest = log.read(first.next_after, 100);
    assert_eq!(numbers(&rest), [3, 4, 5]);
    assert_eq!(rest.next_after, 5);
    // Every line read, and the job still runs: more may come.
    assert!(!rest.complete);
    let nothing = log.read(5, 100);
    assert_eq!((numbers(&nothing), nothing.next_after), (vec![], 5));
    assert!(!nothing.complete);

    // Times never run back, and levels are kept.
    log.line(LogLevel::Error, "the last");
    let all = log.read(0, 100);
    assert!(
        all.lines
            .windows(2)
            .all(|pair| pair[0].at_ms <= pair[1].at_ms)
    );
    assert_eq!(all.lines[5].level, LogLevel::Error);
    assert_eq!(all.lines[0].level, LogLevel::Info);
}

#[test]
fn a_log_is_complete_only_once_it_has_ended_and_its_last_line_is_read() {
    let log = written(LogLimits::default(), 3);
    assert!(!log.has_ended());
    log.end();
    assert!(log.has_ended());
    let part = log.read(0, 2);
    assert!(!part.complete, "a line is still to read");
    let rest = log.read(part.next_after, 2);
    assert_eq!(numbers(&rest), [3]);
    assert!(rest.complete);
    assert!(log.read(3, 2).complete);
    // A cursor past the end reads nothing, and stays where it was.
    let beyond = log.read(9, 2);
    assert_eq!((numbers(&beyond), beyond.next_after), (vec![], 9));
    assert!(beyond.complete);

    // Nothing is added to a log that has ended.
    log.line(LogLevel::Info, "too late");
    log.progress(1, 2);
    let after = log.read(0, 100);
    assert_eq!(numbers(&after), [1, 2, 3]);
    assert_eq!(after.progress, None);
}

#[test]
fn the_newest_lines_are_kept_and_the_dropped_ones_counted() {
    let log = written(limits(3, 1 << 20, 100), 10);
    let all = log.read(0, 100);
    assert_eq!(numbers(&all), [8, 9, 10], "numbers are never used again");
    assert_eq!(all.lines[0].message, "line 8");
    assert_eq!(all.dropped_lines, 7);
    assert_eq!(all.next_after, 10);

    // A cursor inside what was dropped is told how much it missed; one
    // within what is kept misses nothing.
    assert_eq!(log.read(5, 100).dropped_lines, 2);
    assert_eq!(numbers(&log.read(5, 100)), [8, 9, 10]);
    assert_eq!(log.read(7, 100).dropped_lines, 0);
    assert_eq!(log.read(8, 100).dropped_lines, 0);
    assert_eq!(numbers(&log.read(8, 100)), [9, 10]);
    // The count comes with the first page only.
    let page = log.read(0, 1);
    assert_eq!((numbers(&page), page.dropped_lines), (vec![8], 7));
    assert_eq!(log.read(page.next_after, 1).dropped_lines, 0);
}

#[test]
fn the_text_kept_is_bounded_too_and_a_lone_line_always_fits() {
    // Ten bytes a line, and room for 25 bytes of text.
    let log = JobLog::new(limits(100, 25, 100));
    for n in 0..6 {
        log.line(LogLevel::Info, &format!("0123456-{n:02}"));
    }
    let all = log.read(0, 100);
    assert_eq!(numbers(&all), [5, 6], "two lines of ten bytes fit 25");
    assert_eq!(all.dropped_lines, 4);

    // A line longer than the whole log is cut to it, and replaces it.
    log.line(LogLevel::Warning, &"x".repeat(60));
    let all = log.read(0, 100);
    assert_eq!(numbers(&all), [7]);
    assert_eq!(all.lines[0].message, "x".repeat(25));
    assert_eq!(all.lines[0].dropped_bytes, Some(35));
    assert_eq!(all.dropped_lines, 6);

    // A log told to keep no line keeps the newest.
    let none = written(limits(0, 1 << 20, 100), 3);
    assert_eq!(numbers(&none.read(0, 100)), [3]);
}

#[test]
fn a_long_line_is_cut_to_its_start_between_two_characters_and_says_by_how_much() {
    let log = JobLog::new(limits(10, 1 << 20, 8));
    log.line(LogLevel::Info, "12345678");
    log.line(LogLevel::Info, "123456789");
    // `é` is two bytes: the cut falls before it, not inside it.
    log.line(LogLevel::Info, "1234567é9");
    log.line(LogLevel::Info, "");
    let read = log.read(0, 100);
    let lines: Vec<(&str, Option<u64>)> = read
        .lines
        .iter()
        .map(|line| (line.message.as_str(), line.dropped_bytes))
        .collect();
    assert_eq!(
        lines,
        [
            ("12345678", None),
            ("12345678", Some(1)),
            ("1234567", Some(3)),
            ("", None)
        ]
    );
}

#[test]
fn progress_never_passes_its_total_nor_goes_back() {
    let log = JobLog::new(LogLimits::default());
    assert_eq!(log.read(0, 1).progress, None);
    let said = |log: &JobLog| log.read(0, 1).progress.map(|p| (p.bars_done, p.bars_total));
    log.progress(10, 100);
    assert_eq!(said(&log), Some((10, 100)));
    log.progress(5, 100);
    assert_eq!(said(&log), Some((10, 100)), "never back");
    log.progress(250, 100);
    assert_eq!(said(&log), Some((100, 100)), "never past the total");
    // Said with another total, it still never goes back: fewer bars are
    // not taken, as many are with their total, and more are.
    let mut changes = log.changes();
    log.progress(40, 400);
    assert_eq!(
        said(&log),
        Some((100, 100)),
        "never back, whatever the total"
    );
    assert!(!woke(&mut changes), "nothing new");
    log.progress(100, 400);
    assert_eq!(said(&log), Some((100, 400)));
    assert!(woke(&mut changes));
    log.progress(150, 400);
    assert_eq!(said(&log), Some((150, 400)));
    // Progress is no line: it takes no number.
    log.line(LogLevel::Info, "one");
    assert_eq!(numbers(&log.read(0, 10)), [1]);
}

#[test]
fn a_flood_leaves_a_log_within_its_limits_whatever_was_written() {
    let limits = LogLimits::default();
    let log = JobLog::new(limits);
    let long = "y".repeat(3 * limits.max_line_bytes);
    for n in 0..200_000_u64 {
        if n % 1_000 == 0 {
            log.line(LogLevel::Warning, &long);
        } else {
            log.line(
                LogLevel::Info,
                "a line of some sixty bytes, as a strategy would print them",
            );
        }
    }
    let state = log.state();
    assert!(state.kept.lines.len() <= limits.max_lines);
    assert!(state.bytes <= limits.max_bytes);
    let text: usize = state.kept.lines.iter().map(|line| line.message.len()).sum();
    assert_eq!(text, state.bytes, "the count of bytes is the text kept");
    assert!(
        state
            .kept
            .lines
            .iter()
            .all(|line| line.message.len() <= limits.max_line_bytes),
        "no line longer than a line may be"
    );
    assert_eq!(state.kept.last(), 200_000);
    assert_eq!(
        state.kept.dropped_lines,
        200_000 - state.kept.lines.len() as u64
    );
    assert_eq!(state.kept.lines.back().map(|line| line.seq), Some(200_000));
}

#[test]
fn a_log_is_stored_as_its_kept_lines_what_it_dropped_and_its_progress() {
    assert_eq!(JobLog::new(LogLimits::default()).stored_json(), None);

    let log = written(limits(2, 1 << 20, 4), 3);
    log.progress(3, 4);
    let json = log.stored_json().expect("a log");
    let stored: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    assert_eq!(stored["dropped_lines"], 1);
    assert_eq!(
        stored["progress"],
        serde_json::json!({"bars_done": 3, "bars_total": 4})
    );
    let lines = stored["lines"].as_array().expect("lines");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["seq"], 2);
    assert_eq!(lines[0]["message"], "line");
    assert_eq!(lines[0]["dropped_bytes"], 2);
    assert_eq!(lines[0]["level"], "info");

    // Read back, it answers as the log did once it had ended.
    let kept = Kept::from_stored(&json).expect("a stored log");
    log.end();
    for (after, limit) in [(0, 100), (0, 1), (1, 100), (2, 100), (3, 100), (7, 1)] {
        assert_eq!(
            kept.read(after, limit, true),
            log.read(after, limit),
            "after {after}, limit {limit}"
        );
    }

    // Progress alone, or drops alone, are a log worth keeping.
    let progress_only = JobLog::new(LogLimits::default());
    progress_only.progress(1, 2);
    assert_eq!(
        progress_only.stored_json().as_deref(),
        Some(r#"{"dropped_lines":0,"lines":[],"progress":{"bars_done":1,"bars_total":2}}"#)
    );
}

#[test]
fn a_stored_log_no_build_wrote_is_refused() {
    let line = |seq: u64| format!(r#"{{"seq":{seq},"at_ms":1,"level":"info","message":"m"}}"#);
    let stored = |dropped: u64, seqs: &[u64]| {
        let lines: Vec<String> = seqs.iter().map(|seq| line(*seq)).collect();
        format!(
            r#"{{"dropped_lines":{dropped},"lines":[{}]}}"#,
            lines.join(",")
        )
    };
    assert!(Kept::from_stored(&stored(0, &[1, 2, 3])).is_ok());
    assert!(Kept::from_stored(&stored(7, &[8, 9])).is_ok());
    assert!(Kept::from_stored(&stored(4, &[])).is_ok());
    assert_eq!(
        Kept::from_stored(&stored(0, &[2, 3])).expect_err("not from one"),
        "line 1 is numbered 2"
    );
    assert_eq!(
        Kept::from_stored(&stored(1, &[2, 4])).expect_err("a number skipped"),
        "line 3 is numbered 4"
    );
    assert!(Kept::from_stored("[]").is_err());
    assert!(Kept::from_stored("").is_err());
    assert!(
        Kept::from_stored(r#"{"lines":[]}"#).is_err(),
        "no dropped count"
    );
    // A level only a later build names reads, and could never be sent on.
    let later = stored(2, &[3, 4])
        .replacen("info", "trace", 2)
        .replacen("trace", "info", 1);
    assert_eq!(
        Kept::from_stored(&later).expect_err("a level with no name"),
        "line 4 has a level this build does not know"
    );
}

#[test]
fn a_level_with_no_name_is_kept_as_info_so_that_whatever_a_log_holds_can_be_written() {
    let log = JobLog::new(LogLimits::default());
    log.line(LogLevel::Unrecognized, "from a writer that names no level");
    log.line_cut(LogLevel::Unrecognized, "and cut", 3);
    let read = log.read(0, 10);
    assert_eq!(read.lines[0].level, LogLevel::Info);
    assert_eq!(read.lines[1].level, LogLevel::Info);
    assert_eq!(read.lines[1].dropped_bytes, Some(3));
    let stored = log.stored_json().expect("a log that can be stored");
    assert_eq!(
        Kept::from_stored(&stored)
            .expect("the log")
            .read(0, 10, false),
        read
    );
    serde_json::to_string(&read.lines).expect("lines a page can hold");
}

#[test]
fn a_closed_log_takes_no_more_and_tells_its_readers_only_at_its_end() {
    let log = written(LogLimits::default(), 2);
    log.progress(1, 4);
    let mut changes = log.changes();
    log.close();
    assert!(!woke(&mut changes), "nothing a reader would see");
    // Whatever is written to it now is not taken.
    log.line(LogLevel::Error, "too late");
    log.progress(4, 4);
    assert!(!woke(&mut changes));
    let read = log.read(0, 10);
    assert_eq!(numbers(&read), [1, 2]);
    assert_eq!(
        read.progress,
        Some(JobProgress {
            bars_done: 1,
            bars_total: 4
        })
    );
    assert!(!read.complete && !log.has_ended(), "not said yet");
    let stored = log.stored_json().expect("the log to store");

    log.end();
    assert!(woke(&mut changes), "the end");
    let ended = log.read(0, 10);
    assert!(ended.complete && log.has_ended());
    assert_eq!(ended.lines, read.lines);
    assert_eq!(
        log.stored_json(),
        Some(stored),
        "what was stored is the log"
    );
}

/// Whether the log changed since this was last asked.
fn woke(changes: &mut watch::Receiver<u64>) -> bool {
    let changed = changes.has_changed().expect("a log");
    changes.borrow_and_update();
    changed
}

#[test]
fn whoever_follows_a_log_is_woken_by_a_line_by_progress_and_by_its_end() {
    let log = JobLog::new(LogLimits::default());
    let mut changes = log.changes();
    assert!(!woke(&mut changes), "nothing yet");
    log.line(LogLevel::Info, "one");
    assert!(woke(&mut changes), "a line");
    log.progress(1, 2);
    assert!(woke(&mut changes), "progress");
    // Progress that says nothing new wakes no one.
    log.progress(1, 2);
    assert!(!woke(&mut changes));
    log.end();
    assert!(woke(&mut changes), "the end");
    assert!(format!("{log:?}").contains("ended: true"));
}

#[test]
fn a_log_shared_by_threads_numbers_every_line_once() {
    let log = JobLog::new(LogLimits::default());
    std::thread::scope(|scope| {
        for writer in 0..4 {
            let log = log.clone();
            scope.spawn(move || {
                for n in 0..500 {
                    log.line(LogLevel::Info, &format!("writer {writer} line {n}"));
                }
            });
        }
    });
    let all = log.read(0, 10_000);
    assert_eq!(numbers(&all), (1..=2_000).collect::<Vec<u64>>());
}
