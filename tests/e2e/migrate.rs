//! `quantforge data migrate` as an operator runs it, against the acceptance
//! criteria of `docs/storage-migration.md`. Without a server: a SQLite
//! target and a missing source are refused before anything is created or
//! reached. On the server `QUANTFORGE_TEST_POSTGRES_URL` names: the fixture
//! migrates and verifies with the oracle's checksums while the source keeps
//! every byte; a copy cut off, by a refused row or a killed process,
//! converges when run again, and a repeat writes nothing; a copy resumed
//! after the source changed a row it copied is refused before it writes; a
//! planted mismatch exits non-zero with a report naming it; open runs stop
//! a copy until the operator says the source is quiescent; a trace log
//! shows no credential; and every command reads the target afterwards.

use predicates::prelude::*;

use crate::harness::{TestBed, unpinned_cmd};

#[test]
fn a_sqlite_target_is_refused_before_anything_is_created() {
    let bed = TestBed::new();
    unpinned_cmd(&bed)
        .args([
            "--log-level",
            "error",
            "data",
            "migrate",
            "--from",
            "source.sqlite",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "data migrate copies into PostgreSQL: select it with --storage postgres and \
             --postgres-url, or QF_STORAGE and QF_POSTGRES_URL",
        ));
    assert_eq!(bed.created(), Vec::<std::ffi::OsString>::new());
}

/// A build without PostgreSQL refuses `--storage postgres` first.
#[cfg(feature = "postgres")]
#[test]
fn a_missing_source_is_refused_before_the_server_is_reached() {
    let bed = TestBed::new();
    unpinned_cmd(&bed)
        .args(["--log-level", "error", "--storage", "postgres"])
        .env(
            "QF_POSTGRES_URL",
            "postgres://quant:pw@127.0.0.1:9/nothing?sslmode=disable",
        )
        .args(["data", "migrate", "--from", "missing.sqlite"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no SQLite database at missing.sqlite (--from)",
        ))
        .stderr(predicate::str::contains("connect").not());
}

#[cfg(feature = "postgres")]
mod on_a_server {
    use std::path::{Path, PathBuf};

    use quantforge::model::TRANSFER_TABLES;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::backends::real_server::{Role, SENTINEL};
    use crate::harness::{CliOutput, cli_cmd};
    use crate::server::{query, server_url};
    use crate::transfer_fixture::{self, Cases};

    fn sha256(path: &Path) -> String {
        hex::encode(Sha256::digest(
            std::fs::read(path).expect("read the source"),
        ))
    }

    /// The `verified:` lines a verification of the fixture prints.
    fn verified_lines(cases: &Cases) -> Vec<String> {
        cases
            .scopes
            .iter()
            .map(|scope| {
                let key = TRANSFER_TABLES
                    .iter()
                    .find(|table| table.name == scope.table)
                    .expect("a transferred table")
                    .key;
                let market = scope
                    .market
                    .as_ref()
                    .map_or_else(String::new, |market| format!(" {market}"));
                let bounds = match (scope.first, scope.last) {
                    (Some(first), Some(last)) => format!(" {key}={first}..{last}"),
                    _ => String::new(),
                };
                format!(
                    "{}{market} rows={}{bounds} checksum={}",
                    scope.table, scope.rows, scope.checksum
                )
            })
            .collect()
    }

    /// A source in the bed's directory, seeded with the fixture.
    fn seeded(bed: &TestBed) -> (PathBuf, Cases) {
        let source = bed.dir().join("source.sqlite");
        let cases = transfer_fixture::seed(&source);
        (source, cases)
    }

    /// A trigger failing the copy of the closed trade numbered 4.
    const REFUSE_TRADE_4: &str = "CREATE FUNCTION refuse() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.seq = 4 THEN RAISE EXCEPTION 'planted failure'; END IF; RETURN NEW; END $$; \
         CREATE TRIGGER refuse BEFORE INSERT ON closed_trades FOR EACH ROW EXECUTE FUNCTION \
         refuse();";

    fn migrate(bed: &TestBed, source: &Path) -> assert_cmd::Command {
        let mut cmd = cli_cmd(bed, "error");
        cmd.args(["data", "migrate", "--from"]).arg(source);
        cmd
    }

    #[test]
    fn the_fixture_migrates_and_verifies_and_the_source_keeps_every_byte() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let (source, cases) = seeded(&bed);
        let before = sha256(&source);

        let assert = migrate(&bed, &source)
            .args(["--verify", "--batch-size", "2"])
            .assert()
            .success();
        let output = CliOutput::from_assert(&assert);
        assert_eq!(output.get("storage"), Some(bed.storage_text().as_str()));
        assert_eq!(output.get("copy"), Some("started"));
        assert_eq!(output.get_all("verified"), verified_lines(&cases));
        assert_eq!(output.get("differences"), Some("0"));
        assert_eq!(output.get("verify"), Some("ok"));
        let copied: Vec<&str> = output.get_all("copied");
        assert_eq!(copied[0], "candles rows=12 written=12");
        assert_eq!(copied.len(), TRANSFER_TABLES.len());
        assert_eq!(sha256(&source), before);

        let source_id = output.get("source_store_id").expect("the source's ID");
        let status = cli_cmd(&bed, "error")
            .args(["store", "status"])
            .assert()
            .success();
        let status = CliOutput::from_assert(&status);
        assert_eq!(status.get("environment"), Some("testnet"));
        assert_eq!(status.get("activation"), Some("inactive"));
        assert_ne!(status.get("store_id"), Some(source_id));
        let transfer = status.get("transfer").expect("a transfer line");
        assert!(
            transfer.starts_with(&format!("copied from {source_id} at ")),
            "{transfer}"
        );
        assert_eq!(status.get("legacy_runs"), Some("1"));
    }

    #[test]
    fn a_copy_cut_off_converges_when_run_again_and_a_repeat_writes_nothing() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let (source, cases) = seeded(&bed);
        cli_cmd(&bed, "error")
            .args(["store", "status"])
            .assert()
            .success();
        bed.execute(REFUSE_TRADE_4);

        migrate(&bed, &source)
            .args(["--batch-size", "1"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "the copy stopped in closed_trades; the rows it wrote stay, and running it \
                 again finishes it",
            ))
            .stderr(predicate::str::contains("planted failure"));
        cli_cmd(&bed, "error")
            .args(["store", "status"])
            .assert()
            .success()
            .stdout(predicate::str::contains("transfer: copying from "));
        cli_cmd(&bed, "error")
            .args(["data", "validate", "--symbol", "BTCUSDT"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "holds an unfinished copy of store",
            ));

        bed.execute("DROP TRIGGER refuse ON closed_trades");
        let resumed = migrate(&bed, &source)
            .args(["--verify", "--batch-size", "1"])
            .assert()
            .success();
        let resumed = CliOutput::from_assert(&resumed);
        assert_eq!(resumed.get("copy"), Some("resumed"));
        assert!(
            resumed
                .get_all("copied")
                .contains(&"closed_trades rows=2 written=1")
        );
        assert_eq!(resumed.get_all("verified"), verified_lines(&cases));

        let again = migrate(&bed, &source).arg("--verify").assert().success();
        let again = CliOutput::from_assert(&again);
        assert_eq!(again.get("copy"), Some("already copied"));
        assert_eq!(again.get_all("copied"), Vec::<&str>::new());
        assert_eq!(again.get_all("verified"), verified_lines(&cases));
    }

    #[test]
    fn a_copy_resumed_after_the_source_changed_a_copied_row_is_refused_before_it_writes() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let (source, _) = seeded(&bed);
        cli_cmd(&bed, "error")
            .args(["store", "status"])
            .assert()
            .success();
        bed.execute(REFUSE_TRADE_4);
        migrate(&bed, &source)
            .args(["--batch-size", "1"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "the copy stopped in closed_trades",
            ));
        bed.execute("DROP TRIGGER refuse ON closed_trades");
        // Between the attempts, a candle the first one copied is corrected.
        rusqlite::Connection::open(&source)
            .expect("open the source")
            .execute(
                "UPDATE candles SET close = '67013.50' WHERE close = '67013.5'",
                [],
            )
            .expect("correct a candle");
        let url = bed.postgres_url().expect("a URL").to_string();
        let trades = || query(&url, "SELECT count(*) FROM closed_trades")[0][0].clone();
        let before = trades();

        migrate(&bed, &source)
            .args(["--batch-size", "1"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "transfer refused: the source changed since this copy began: the target holds \
                 1 row(s) the source has since changed or deleted (first: candles binance_spot \
                 BTCUSDT 1m open_time_ms=1792454520000: close differs: source \"67013.50\", \
                 target \"67013.5\"); a copy never rewrites a row it holds, so this one cannot \
                 be finished, but the source is untouched: give the copy a new, empty database \
                 and run the command again",
            ));
        assert_eq!(trades(), before);
        cli_cmd(&bed, "error")
            .args(["store", "status"])
            .assert()
            .success()
            .stdout(predicate::str::contains("transfer: copying from "));
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_killed_midway_converges_when_run_again() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let (source, _) = seeded(&bed);
        rusqlite::Connection::open(&source)
            .expect("open the source")
            .execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 19999) \
                 INSERT INTO candles SELECT 'binance_spot', 'MANYUSDT', '1m', i * 60000, \
                 i * 60000 + 59999, '1.0', '1.1', '0.9', '1.05', '2.50', i FROM n;",
            )
            .expect("many candles");
        let url = bed.postgres_url().expect("a URL").to_string();
        let mut child = crate::harness::isolated_std_cmd()
            .current_dir(bed.dir())
            .args(["--log-level", "error", "--storage", "postgres"])
            .env("QF_POSTGRES_URL", &url)
            .args(["data", "migrate", "--batch-size", "10", "--from"])
            .arg(&source)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start the copy");
        let started = std::time::Instant::now();
        let has = |sql: &str| query(&url, sql)[0][0].as_deref() == Some("t");
        while !(has("SELECT EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'candles')")
            && has("SELECT EXISTS (SELECT 1 FROM candles)"))
        {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(60),
                "the copy never began"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child.kill().expect("kill the copy");
        child.wait().expect("the copy ended");

        let resumed = migrate(&bed, &source).arg("--verify").assert().success();
        let resumed = CliOutput::from_assert(&resumed);
        assert_eq!(resumed.get("copy"), Some("resumed"));
        assert_eq!(resumed.get("verify"), Some("ok"));
        assert!(
            resumed
                .get_all("verified")
                .iter()
                .any(|line| line.starts_with("candles binance_spot MANYUSDT 1m rows=20000 ")),
            "{:?}",
            resumed.get_all("verified")
        );
    }

    #[test]
    fn a_deliberate_mismatch_fails_verification_with_a_report_naming_it() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let (source, _) = seeded(&bed);
        migrate(&bed, &source).assert().success();
        bed.execute(
            "UPDATE candles SET close = '67013.50' WHERE close = '67013.5'; DELETE FROM \
             order_events WHERE seq = 9;",
        );
        let before = sha256(&source);

        migrate(&bed, &source)
            .arg("--verify")
            .assert()
            .failure()
            .stdout(predicate::str::contains(
                "mismatch: candles binance_spot BTCUSDT 1m source rows=5 ",
            ))
            .stdout(predicate::str::contains(
                "mismatch: order_events source rows=5 seq=1..10 checksum=sha256:71417ceba37e",
            ))
            .stdout(predicate::str::contains("differences: 2\n"))
            .stdout(predicate::str::contains(
                "  candles binance_spot BTCUSDT 1m open_time_ms=1792454520000: close differs: \
                 source \"67013.5\", target \"67013.50\"\n",
            ))
            .stdout(predicate::str::contains(
                "  order_events seq=9: only in the source\n",
            ))
            .stdout(predicate::str::contains("verify: failed"))
            .stderr(predicate::str::contains(
                "verification failed: the target differs from the source in 2 row(s), listed \
                 above; the source is untouched, so a new copy into an empty database starts \
                 over cleanly",
            ));
        assert_eq!(sha256(&source), before);
    }

    #[test]
    fn open_runs_stop_a_copy_until_the_operator_says_the_source_is_quiescent() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let (source, _) = seeded(&bed);
        rusqlite::Connection::open(&source)
            .expect("open the source")
            .execute(
                "UPDATE bot_runs SET status = 'running' WHERE run_id = 'run-a'",
                [],
            )
            .expect("a run left running");

        migrate(&bed, &source)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "the source has runs that may still be writing to it: run-a (running); stop \
                 them first, or, once you have checked that nothing writes to the source, run \
                 again with --assume-quiescent",
            ));
        cli_cmd(&bed, "error")
            .args(["store", "status"])
            .assert()
            .success()
            .stdout(predicate::str::contains("transfer: none"))
            .stdout(predicate::str::contains("environment: undeclared"));

        migrate(&bed, &source)
            .args(["--assume-quiescent", "--verify"])
            .assert()
            .success()
            .stdout(predicate::str::contains("verify: ok"));
    }

    /// A role with no rights but owning its database, its password a word
    /// no output holds by chance: a trace log shows everything else.
    #[test]
    fn a_trace_log_of_a_migration_shows_no_credential() {
        let Some(server) = server_url() else {
            return;
        };
        let role = Role::create(&server);
        let bed = TestBed::on_server(&server);
        let mut url = url::Url::parse(bed.postgres_url().expect("a URL")).expect("the URL");
        let database = url.path().trim_start_matches('/').to_string();
        query(
            &server,
            &format!("ALTER DATABASE {database} OWNER TO {}", role.name),
        );
        url.set_username(&role.name).expect("a user");
        url.set_password(Some(SENTINEL)).expect("a password");
        let (source, _) = seeded(&bed);

        let output = unpinned_cmd(&bed)
            .args(["--log-level", "trace", "--storage", "postgres"])
            .env("QF_POSTGRES_URL", url.as_str())
            .args(["data", "migrate", "--verify", "--from"])
            .arg(&source)
            .output()
            .expect("run quantforge");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "the copy failed: {text}");
        assert!(!text.contains(SENTINEL), "the password leaked");
        assert!(!text.contains(&role.name), "the user leaked");
        assert!(text.contains("copied"), "no progress was logged");
    }

    #[test]
    fn every_command_reads_the_target_as_it_read_the_source() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let (source, _) = seeded(&bed);
        migrate(&bed, &source).assert().success();

        // The fixture's spot market skips a day: both stores say so alike.
        let validate = ["data", "validate", "--symbol", "BTCUSDT"];
        let on_target = cli_cmd(&bed, "error")
            .args(validate)
            .output()
            .expect("validate the target");
        let on_source = unpinned_cmd(&bed)
            .args(["--log-level", "error", "--db"])
            .arg(&source)
            .args(validate)
            .output()
            .expect("validate the source");
        assert_eq!(on_target.stdout, on_source.stdout);
        assert_eq!(on_target.status.code(), on_source.status.code());
        let shown = String::from_utf8_lossy(&on_target.stdout);
        assert!(shown.contains("candles: 5\nissues: 1\n"), "{shown}");

        cli_cmd(&bed, "error")
            .args(["--base-url", "https://testnet.binance.vision/"])
            .args(["store", "adopt-run", "--run-id", "run-legacy"])
            .args(["--execution-mode", "dry-run", "--yes"])
            .assert()
            .success();
        cli_cmd(&bed, "error")
            .args(["store", "status"])
            .assert()
            .success()
            .stdout(predicate::str::contains("legacy_runs: 0"));
    }
}
