//! Every command that reads or writes the store, on whichever backend is
//! selected. Each scenario runs on SQLite and on a real PostgreSQL through
//! [`on_each_backend!`], against the mock exchange, so one set of assertions
//! holds on both; [`crate::offline`] runs its own the same way.
//!
//! The tests after the scenarios need a real server: the two backends
//! compared answer for answer, and what an operator sees when PostgreSQL
//! refuses a connection or cannot keep its commits.

use assert_cmd::Command;
use predicates::prelude::*;
use quantforge::{
    BotRunState, CandleQuery, CandleStore, ExchangeId, ExecutionMode, IdentitySource, Interval,
    MarketId, PositionState, RunIdentity, RunJournalStore, RunStatus, Symbol,
};
use serde_json::Value;

use crate::harness::{CliOutput, TestBed, cli_cmd, offline_cmd, on_each_backend};
use crate::mock_binance::{
    MockBinance, Scenario, account_response, exchange_info_btcusdt, kline_row,
};
use crate::offline::anchor_ms;

/// 2024-01-01T00:00:00Z: fixed and long past.
const DAY_START: i64 = 1_704_067_200_000;
const MINUTE: i64 = 60_000;

const SYNC: [&str; 10] = [
    "data",
    "sync",
    "--symbol",
    "BTCUSDT",
    "--interval",
    "1m",
    "--start",
    "2024-01-01T00:00:00Z",
    "--end",
    "2024-01-01T00:05:00Z",
];
const VALIDATE: [&str; 6] = [
    "data",
    "validate",
    "--symbol",
    "BTCUSDT",
    "--interval",
    "1m",
];
/// The strategy's default periods would never trade on five bars.
const BACKTEST: [&str; 9] = [
    "backtest",
    "--symbol",
    "BTCUSDT",
    "--interval",
    "1m",
    "--fast",
    "1",
    "--slow",
    "2",
];
const MONITOR_STATUS: [&str; 4] = ["monitor", "status", "--symbol", "BTCUSDT"];
const MONITOR_WATCH: [&str; 6] = [
    "monitor",
    "watch",
    "--symbol",
    "BTCUSDT",
    "--max-loops",
    "1",
];

fn market(exchange: ExchangeId) -> MarketId {
    MarketId::new(
        exchange,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn everything() -> CandleQuery {
    CandleQuery {
        start_time_ms: None,
        end_time_ms: None,
        limit: None,
    }
}

/// Runs `cmd`, which must succeed, and parses what it printed.
fn answer(cmd: &mut Command) -> CliOutput {
    CliOutput::from_assert(&cmd.assert().success())
}

/// Five closed bars that rise and fall, so a fast average crosses a slow one
/// both ways.
fn five_bars() -> Vec<Value> {
    let closes = [
        "100.00000000",
        "100.00000000",
        "120.00000000",
        "130.00000000",
        "90.50000000",
    ];
    (0i64..)
        .zip(closes)
        .map(|(index, close)| kline_row(DAY_START + index * MINUTE, close))
        .collect()
}

/// A bot's market data: rules, two silent warm-up bars, then a bar up and a
/// bar down, which enter and leave with `--fast 1 --slow 2`.
fn script_a_run(scenario: &mut Scenario, anchor: i64) {
    scenario.exchange_info(exchange_info_btcusdt());
    scenario.klines_page(vec![
        kline_row(anchor, "100.00000000"),
        kline_row(anchor + MINUTE, "100.00000000"),
    ]);
    scenario.klines_page(vec![kline_row(anchor + 2 * MINUTE, "110.00000000")]);
    scenario.klines_page(vec![]);
    scenario.klines_page(vec![kline_row(anchor + 3 * MINUTE, "90.00000000")]);
}

/// All that one `monitor status` asks the exchange.
fn script_a_report(scenario: &mut Scenario) {
    scenario.exchange_info(exchange_info_btcusdt());
    scenario.account(account_response("BTC", "0.05000000"));
    scenario.open_orders(serde_json::json!([]));
    scenario.my_trades(serde_json::json!([]));
}

fn dry_run(bed: &TestBed, mock: &MockBinance, run_id: &str, slow: &str) -> Command {
    let mut cmd = offline_cmd(bed, mock);
    cmd.args(["trade", "run", "--symbol", "BTCUSDT", "--interval", "1m"])
        .args(["--mode", "dry-run", "--fast", "1", "--slow", slow])
        .args(["--bootstrap-bars", "5", "--poll-secs", "1", "--max-loops"])
        .args(["2", "--quote-order-qty", "100", "--run-id", run_id]);
    cmd
}

fn synced_candles_are_what_validate_and_backtest_read(bed: &TestBed) {
    let mut scenario = Scenario::new();
    scenario.klines_page(five_bars());
    let mock = MockBinance::start(scenario);

    let synced = answer(offline_cmd(bed, &mock).args(SYNC));
    assert_eq!(synced.get("written"), Some("5"));
    assert_eq!(synced.get("last_open_time"), Some("2024-01-01T00:04:00Z"));

    let validated = answer(offline_cmd(bed, &mock).args(VALIDATE));
    assert_eq!(validated.get("market"), Some("binance_spot BTCUSDT 1m"));
    assert_eq!(validated.get("candles"), Some("5"));
    assert_eq!(validated.get("issues"), Some("0"));

    let tested = answer(offline_cmd(bed, &mock).args(BACKTEST));
    assert_eq!(tested.get("strategy"), Some("sma_cross"));
    assert_eq!(tested.get("trade_count"), Some("1"));

    // The text the exchange sent, under the exchange's own key and no other.
    let store = bed.store();
    let stored = store
        .load_candles(&market(ExchangeId::BinanceSpot), everything())
        .expect("candles");
    assert_eq!(stored.len(), 5);
    assert_eq!(stored[4].close.to_string(), "90.50000000");
    let other = store
        .load_candles(&market(ExchangeId::BybitSpot), everything())
        .expect("candles");
    assert!(other.is_empty(), "{other:?}");
}

fn a_dry_run_keeps_its_journal_in_the_store_and_monitor_reads_it_back(bed: &TestBed) {
    let mut scenario = Scenario::new();
    script_a_run(&mut scenario, anchor_ms());
    script_a_report(&mut scenario);
    script_a_report(&mut scenario);
    let mock = MockBinance::start(scenario);

    let finished = answer(&mut dry_run(bed, &mock, "e2e-dry-1", "2"));
    assert_eq!(finished.get("run_id"), Some("e2e-dry-1"));
    assert_eq!(finished.get("processed_bars"), Some("2"));
    assert_eq!(finished.get("submitted_orders"), Some("2"));
    assert_eq!(finished.get("closed_trades"), Some("1"));

    let store = bed.store();
    let state = store
        .load_run_state("e2e-dry-1")
        .expect("load run")
        .expect("run state");
    assert_eq!(state.market, market(ExchangeId::BinanceSpot));
    assert_eq!(state.execution_mode, ExecutionMode::DryRun);
    assert_eq!(state.status, RunStatus::Stopped);
    let events = store.list_order_events("e2e-dry-1", 10).expect("events");
    assert_eq!(events.len(), 2);
    let trades = store.list_closed_trades("e2e-dry-1", 10).expect("trades");
    assert_eq!(trades.len(), 1);
    let stored = store
        .load_candles(&market(ExchangeId::BinanceSpot), everything())
        .expect("candles");
    assert_eq!(stored.len(), 4);

    for command in [&MONITOR_STATUS[..], &MONITOR_WATCH[..]] {
        let report = answer(offline_cmd(bed, &mock).args(command));
        assert_eq!(
            report.get("latest_run_id"),
            Some("e2e-dry-1"),
            "{command:?}"
        );
        assert_eq!(report.get("run_identity"), Some("recorded"), "{command:?}");
        assert_eq!(report.get("run_status"), Some("stopped"), "{command:?}");
        assert_eq!(report.get("position_qty"), Some("0"), "{command:?}");
    }
}

/// A run keeps the identity it started under through every later command:
/// its own strategy, and an environment that was never declared.
fn a_run_resumes_only_as_what_recorded_it(bed: &TestBed) {
    let mut scenario = Scenario::new();
    script_a_run(&mut scenario, anchor_ms());
    scenario.exchange_info(exchange_info_btcusdt());
    scenario.exchange_info(exchange_info_btcusdt());
    let mock = MockBinance::start(scenario);
    answer(&mut dry_run(bed, &mock, "e2e-resume", "2"));

    dry_run(bed, &mock, "e2e-resume", "3")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "run e2e-resume was recorded with strategy config",
        ));

    answer(offline_cmd(bed, &mock).args(["store", "bind", "--environment", "testnet", "--yes"]));
    dry_run(bed, &mock, "e2e-resume", "2")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "environment was recorded as undeclared, but this invocation is testnet",
        ));
}

/// A run 0.2.0 left behind, as a migration marks it: no identity, and a
/// position nothing may touch until an operator adopts it.
fn seed_legacy_run(bed: &TestBed, run_id: &str) {
    let state = BotRunState {
        run_id: run_id.to_string(),
        market: market(ExchangeId::BinanceSpot),
        strategy_name: "sma_cross".to_string(),
        strategy_config: serde_json::json!({"kind": "sma_cross", "fast": 20, "slow": 50}),
        execution_mode: ExecutionMode::DryRun,
        status: RunStatus::Stopped,
        last_processed_open_time_ms: None,
        started_at_ms: DAY_START,
        updated_at_ms: DAY_START,
        stopped_at_ms: Some(DAY_START),
        last_error: None,
        position: PositionState::flat(),
    };
    bed.store()
        .create_run(&state, &RunIdentity::recorded(None, None))
        .expect("create run");
    bed.execute(&format!(
        "UPDATE bot_runs SET identity_source = 'legacy', identity_at_ms = NULL \
         WHERE run_id = '{run_id}'"
    ));
}

fn the_store_commands_read_and_change_the_selected_store(bed: &TestBed) {
    let mock = MockBinance::start(Scenario::new());
    seed_legacy_run(bed, "e2e-legacy");

    let before = answer(cli_cmd(bed, "error").args(["store", "status"]));
    assert_eq!(before.get("storage"), Some(bed.storage_text().as_str()));
    let schema = quantforge::storage::sqlite::SCHEMA_VERSION.to_string();
    assert_eq!(before.get("schema_version"), Some(schema.as_str()));
    assert_eq!(before.get("environment"), Some("undeclared"));
    assert_eq!(before.get("legacy_runs"), Some("1"));
    let store_id = before.get("store_id").expect("the store's ID");

    let bound = answer(offline_cmd(bed, &mock).args([
        "store",
        "bind",
        "--environment",
        "testnet",
        "--yes",
    ]));
    assert_eq!(bound.get("store_id"), Some(store_id));

    let adopted = offline_cmd(bed, &mock)
        .args(["store", "adopt-run", "--run-id", "e2e-legacy", "--yes"])
        .assert()
        .success();
    assert_eq!(
        CliOutput::from_assert(&adopted).get("run_id"),
        Some("e2e-legacy")
    );

    let after = answer(cli_cmd(bed, "error").args(["store", "status"]));
    assert_eq!(after.get("store_id"), Some(store_id));
    assert_eq!(after.get("environment"), Some("testnet"));
    assert_eq!(after.get("legacy_runs"), Some("0"));
    let identity = bed
        .store()
        .run_identity("e2e-legacy")
        .expect("load identity")
        .expect("an identity");
    assert_eq!(identity.source, IdentitySource::Adopted);

    // What the store is bound to reaches the commands that talk to an
    // exchange: the default production endpoint is refused before a request,
    // with a remedy this backend can take.
    let remedy = match bed.postgres_url() {
        Some(_) => "or use another PostgreSQL database",
        None => "or use another --db",
    };
    cli_cmd(bed, "error")
        .args(["--api-key", "key", "--api-secret", "secret"])
        .args(["monitor", "orders", "--symbol", "BTCUSDT"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is bound to testnet"))
        .stderr(predicate::str::contains("refusing before any request"))
        .stderr(predicate::str::contains(remedy));
    offline_cmd(bed, &mock)
        .args(["store", "bind", "--environment", "demo", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(remedy.replace("or ", "so ")));
}

/// A store that was just created may be the wrong one, so the first command
/// says so, and only the first.
fn a_new_store_is_announced_once(bed: &TestBed) {
    let warning = match bed.postgres_url() {
        Some(_) => "database held no QuantForge schema and was created empty",
        None => "database did not exist and was created empty",
    };
    for (first, shown) in [(true, true), (false, false)] {
        let output = cli_cmd(bed, "warn")
            .args(["store", "status"])
            .output()
            .expect("run quantforge");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.contains(warning), shown, "first: {first}\n{stdout}");
    }
}

/// A command that prints no `storage:` line still says where it works.
fn every_command_logs_the_store_it_uses(bed: &TestBed) {
    let output = cli_cmd(bed, "info")
        .args(VALIDATE)
        .output()
        .expect("run quantforge");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("using storage"), "{stdout}");
    assert!(stdout.contains(&bed.storage_text()), "{stdout}");
}

on_each_backend!(
    a_new_store_is_announced_once,
    every_command_logs_the_store_it_uses,
    synced_candles_are_what_validate_and_backtest_read,
    a_dry_run_keeps_its_journal_in_the_store_and_monitor_reads_it_back,
    a_run_resumes_only_as_what_recorded_it,
    the_store_commands_read_and_change_the_selected_store,
);

#[cfg(feature = "postgres")]
pub(crate) mod real_server {
    use quantforge::storage::segments;
    use quantforge::{Candle, SegmentPlan, SegmentStore};
    use url::Url;

    use super::*;
    use crate::harness::{offline_cmd_with_log, unpinned_cmd};
    use crate::server::{query, server_url};

    pub(crate) const SENTINEL: &str = "pw-sentinel-417";
    const NO_SUCH_ROLE: &str = "qf_no_such_role";
    const WEAK_SERVER: &str = "QUANTFORGE_TEST_POSTGRES_WEAK_URL";

    /// What a store told an operator and kept, the clock aside.
    struct Story {
        outputs: Vec<String>,
        closed_trades: Vec<[String; 4]>,
        plan: SegmentPlan,
        packed: Vec<Candle>,
    }

    /// A day of closed bars and a minute of the next, then the commands that
    /// read them, then a dry run and its report.
    fn tell(bed: &TestBed, anchor: i64) -> Story {
        let mut scenario = Scenario::new();
        scenario.klines_page(
            (0..1441)
                .map(|minute| {
                    let close = format!("{}.{:08}", 100 + minute % 50, minute);
                    kline_row(DAY_START + minute * MINUTE, &close)
                })
                .collect(),
        );
        script_a_run(&mut scenario, anchor);
        script_a_report(&mut scenario);
        let mock = MockBinance::start(scenario);

        let mut outputs = Vec::new();
        let mut keep = |mut cmd: Command| {
            let output = cmd.output().expect("run the command");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout}");
            let lines: Vec<&str> = stdout
                .lines()
                .filter(|line| !line.starts_with("time_utc_ms:"))
                .collect();
            outputs.push(lines.join("\n"));
        };
        let mut sync = offline_cmd(bed, &mock);
        sync.args(&SYNC[..6])
            .args(["--start", "2024-01-01T00:00:00Z"])
            .args(["--end", "2024-01-02T00:00:00Z"]);
        keep(sync);
        for command in [&VALIDATE[..], &BACKTEST[..]] {
            let mut cmd = offline_cmd(bed, &mock);
            cmd.args(command);
            keep(cmd);
        }
        keep(dry_run(bed, &mock, "e2e-story", "2"));
        let mut report = offline_cmd(bed, &mock);
        report.args(MONITOR_STATUS);
        keep(report);

        let store = bed.store();
        let closed_trades = store
            .list_closed_trades("e2e-story", 10)
            .expect("trades")
            .into_iter()
            .map(|trade| {
                [
                    trade.qty.to_string(),
                    trade.entry_price.to_string(),
                    trade.exit_price.to_string(),
                    trade.gross_quote_pnl.to_string(),
                ]
            })
            .collect();
        let plan = store
            .resolve_segments(
                &market(ExchangeId::BinanceSpot),
                DAY_START,
                DAY_START + 1440 * MINUTE,
            )
            .expect("a plan");
        let hash = &plan.segments.first().expect("the day's segment").hash;
        let bytes = store
            .load_segment(hash)
            .expect("read the segment")
            .expect("the segment's bytes");
        let packed = segments::open(&bytes, hash).expect("a segment that matches its name");
        Story {
            outputs,
            closed_trades,
            plan,
            packed,
        }
    }

    #[test]
    fn both_backends_tell_the_same_story() {
        let Some(postgres) = TestBed::on_postgres() else {
            return;
        };
        let anchor = anchor_ms();
        let on_sqlite = tell(&TestBed::new(), anchor);
        let on_postgres = tell(&postgres, anchor);
        assert_eq!(on_postgres.outputs, on_sqlite.outputs);
        assert_eq!(on_postgres.closed_trades, on_sqlite.closed_trades);
        assert_eq!(on_postgres.plan, on_sqlite.plan);
        assert_eq!(on_postgres.packed, on_sqlite.packed);
        assert_eq!(on_sqlite.packed.len(), 1440);
        assert_eq!(on_sqlite.closed_trades.len(), 1);
        assert_eq!(postgres.created().len(), 0);
    }

    /// Anything that opened the default SQLite path would choke on this, and
    /// a command on PostgreSQL neither reads it nor changes it.
    #[test]
    fn a_file_at_the_default_sqlite_path_is_never_opened() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let path = bed.dir().join("data").join("market.sqlite");
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
        std::fs::write(&path, b"not a database").expect("the file");

        answer(cli_cmd(&bed, "error").args(["store", "status"]));
        assert_eq!(std::fs::read(&path).expect("the file"), b"not a database");
        assert_eq!(
            std::fs::read_dir(path.parent().expect("a directory"))
                .expect("the directory")
                .count(),
            1,
            "a journal appeared beside it"
        );
    }

    /// `store status` on `url`, refused: the bed stays empty, and what it
    /// said is returned once it is known to show neither user nor password.
    fn refused(url: &Url) -> String {
        let bed = TestBed::new();
        let output = unpinned_cmd(&bed)
            .args(["--log-level", "error", "--storage", "postgres"])
            .env("QF_POSTGRES_URL", url.as_str())
            .args(["store", "status"])
            .output()
            .expect("run quantforge");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains(SENTINEL), "the password leaked");
        assert!(!text.contains(NO_SUCH_ROLE), "the user leaked");
        assert_eq!(output.status.code(), Some(1), "{text}");
        assert!(bed.created().is_empty(), "{:?}", bed.created());
        text
    }

    #[test]
    fn a_connection_the_server_refuses_says_why_and_never_falls_back() {
        let Some(server) = server_url() else {
            return;
        };
        let reachable = Url::parse(&server).expect("the server's URL");
        let host = format!(
            "{}:{}",
            reachable.host_str().expect("a host"),
            reachable.port().unwrap_or(5432)
        );

        // No such role, so it is refused however the server authenticates.
        let mut no_role = reachable.clone();
        no_role.set_username(NO_SUCH_ROLE).expect("a user");
        no_role.set_password(Some(SENTINEL)).expect("a password");
        let text = refused(&no_role);
        assert!(
            text.contains(&format!(
                "failed to initialize postgres store at postgres://{host}{}",
                reachable.path()
            )),
            "{text}"
        );
        assert!(
            text.contains("the server refused the URL's user or password"),
            "{text}"
        );

        let mut no_database = reachable.clone();
        no_database.set_path("/qf_no_such_database");
        let text = refused(&no_database);
        assert!(
            text.contains("the database the URL names does not exist"),
            "{text}"
        );
    }

    /// A login role of its own on a server, dropped with it, so declare it
    /// before the bed whose database it owns. A role belongs to the server,
    /// not to a database, so each takes a name of its own: the process's ID
    /// and a count.
    pub(crate) struct Role {
        server: String,
        pub(crate) name: String,
    }

    impl Role {
        pub(crate) fn create(server: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let name = format!("qf_e2e_owner_{}_{n}", std::process::id());
            query(
                server,
                &format!("CREATE ROLE {name} LOGIN PASSWORD '{SENTINEL}'"),
            );
            Self {
                server: server.to_string(),
                name,
            }
        }
    }

    impl Drop for Role {
        fn drop(&mut self) {
            // Never a second panic while a failed test unwinds.
            let drop = format!("DROP ROLE IF EXISTS {}", self.name);
            let _ = std::panic::catch_unwind(|| query(&self.server, &drop));
        }
    }

    /// The setup `docs/configuration.md` gives: a role with no rights but
    /// owning its database. Its password is a word no output holds by chance,
    /// so a trace log, which shows everything else, can be held to it.
    #[test]
    fn a_role_that_owns_its_database_is_enough_and_a_trace_log_shows_no_credential() {
        let Some(server) = server_url() else {
            return;
        };
        let role = Role::create(&server);
        let bed = TestBed::on_server(&server);
        let mut url = Url::parse(bed.postgres_url().expect("a URL")).expect("the URL");
        let database = url.path().trim_start_matches('/').to_string();
        query(
            &server,
            &format!("ALTER DATABASE {database} OWNER TO {}", role.name),
        );
        url.set_username(&role.name).expect("a user");
        url.set_password(Some(SENTINEL)).expect("a password");

        let output = unpinned_cmd(&bed)
            .args(["--log-level", "trace", "--storage", "postgres"])
            .env("QF_POSTGRES_URL", url.as_str())
            .args(["store", "status"])
            .output()
            .expect("run quantforge");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains(SENTINEL), "the password leaked");
        assert!(!text.contains(&role.name), "the user leaked");
        assert!(output.status.success(), "the status failed: {text}");
        assert!(text.contains(&bed.storage_text()), "no storage line");
    }

    /// One server with `fsync` off: a command that sends orders stops before
    /// any request, and any other runs and warns.
    #[test]
    fn a_live_command_refuses_a_server_that_cannot_keep_its_commits() {
        let Some(server) = crate::server::url(WEAK_SERVER) else {
            eprintln!("skipping: {WEAK_SERVER} is unset");
            return;
        };
        let bed = TestBed::on_server(&server);
        let mock = MockBinance::start(Scenario::new());

        let commands: [&[&str]; 2] = [
            &[
                "trade", "run", "--symbol", "BTCUSDT", "--mode", "live", "--yes",
            ],
            &["trade", "close", "--symbol", "BTCUSDT", "--yes"],
        ];
        for command in commands {
            offline_cmd(&bed, &mock)
                .args(command)
                .assert()
                .failure()
                .stderr(predicate::str::contains(
                    "storage cannot keep its commits: fsync is off, and needs on",
                ));
        }
        assert!(mock.requests().is_empty(), "{:?}", mock.requests());

        offline_cmd_with_log(&bed, &mock, "warn")
            .args(["store", "status"])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "this store's commits may not survive a crash, and live trading refuses it",
            ))
            .stdout(predicate::str::contains("durability: weakened (fsync=off"));
    }

    /// The server keeps its store for as long as it runs, and the store it
    /// keeps is the one selected.
    #[test]
    fn serve_run_holds_a_connection_to_the_selected_database() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let url = bed.postgres_url().expect("a URL");
        let _served = crate::serve::Served::start(&bed);
        let held = query(
            url,
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND application_name = 'quantforge'",
        );
        assert_ne!(held[0][0].as_deref(), Some("0"), "no connection held");
    }
}
