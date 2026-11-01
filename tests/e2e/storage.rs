//! Storage selection: `--storage` and the PostgreSQL settings come from the
//! flag, then the `QF_*` variable, then the default, as
//! `docs/configuration.md` states. An omitted backend is SQLite at `--db`;
//! a PostgreSQL setting beside an omitted backend, and a SQLite path beside
//! PostgreSQL, are refused; PostgreSQL, once selected, is never replaced by
//! SQLite: until a release registers its stores it is refused, and nothing
//! is created.
//!
//! The URLs carry a sentinel password, and no run may print it: [`run`]
//! checks every output, failing or not, without showing it.

use crate::harness::{TestBed, unpinned_cmd};
use assert_cmd::Command;

const STORE_STATUS: [&str; 2] = ["store", "status"];
const UNROUTABLE_URL: &str = "http://127.0.0.1:9/";
const DEFAULT_DB: &str = "data/market.sqlite";
const SENTINEL: &str = "pw-sentinel-417";

/// A PostgreSQL URL with the sentinel password; `query` may make it
/// unusable.
fn postgres_url(query: &str) -> String {
    format!("postgres://quant:{SENTINEL}@db.example.com:5432/market{query}")
}

/// Refused, whatever the build: plain text to a remote host.
fn bad_url() -> String {
    postgres_url("?sslmode=disable")
}

fn good_url() -> String {
    postgres_url("")
}

/// Every setting but the storage ones pinned.
fn storage_cmd(bed: &TestBed) -> Command {
    let mut cmd = unpinned_cmd(bed);
    cmd.args(["--log-level", "error", "--base-url", UNROUTABLE_URL]);
    cmd
}

/// Runs `cmd` and returns its exit code with stdout and stderr together,
/// after checking that neither shows the password or the URL's userinfo.
/// A leak fails without printing the output.
fn run(cmd: &mut Command) -> (Option<i32>, String) {
    let output = cmd.output().expect("run quantforge");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!text.contains(SENTINEL), "the password leaked");
    assert!(!text.contains("quant:"), "the URL's userinfo leaked");
    (output.status.code(), text)
}

/// Runs `cmd`, expecting exit status 1 with `message`, and nothing left in
/// the bed: no SQLite file and no directory for one.
fn refused(bed: &TestBed, cmd: &mut Command, message: &str) {
    let (code, text) = run(cmd);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains(message), "expected {message:?} in:\n{text}");
    assert_nothing_created(bed);
}

fn assert_nothing_created(bed: &TestBed) {
    let created: Vec<_> = std::fs::read_dir(bed.dir())
        .expect("read the bed")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert!(created.is_empty(), "created {created:?}");
}

/// The refusal of a PostgreSQL selection whose settings are valid.
const POSTGRES_REFUSAL: &str = if cfg!(feature = "postgres") {
    "postgres storage has no adapter yet"
} else {
    "postgres storage is not in this build"
};

#[test]
fn an_omitted_backend_is_sqlite_at_qf_db_or_at_the_default_path() {
    let bed = TestBed::new();
    let (code, text) = run(storage_cmd(&bed).args(STORE_STATUS));
    assert_eq!(code, Some(0), "{text}");
    assert!(bed.dir().join(DEFAULT_DB).is_file());

    let bed = TestBed::new();
    let (code, text) = run(storage_cmd(&bed)
        .env("QF_DB", bed.dir().join("from-env.sqlite"))
        .args(STORE_STATUS));
    assert_eq!(code, Some(0), "{text}");
    assert!(bed.dir().join("from-env.sqlite").is_file());
    assert!(!bed.dir().join("data").exists());
}

#[test]
fn the_storage_flag_beats_qf_storage_and_qf_storage_beats_the_default() {
    for flag_first in [true, false] {
        let bed = TestBed::new();
        let mut cmd = storage_cmd(&bed);
        cmd.env("QF_STORAGE", "postgres")
            .env("QF_POSTGRES_URL", good_url());
        if flag_first {
            cmd.args(["--storage", "sqlite"]).args(STORE_STATUS);
        } else {
            cmd.args(STORE_STATUS).args(["--storage", "sqlite"]);
        }
        let (code, text) = run(&mut cmd);
        assert_eq!(code, Some(0), "flag first: {flag_first}\n{text}");
        assert!(bed.dir().join(DEFAULT_DB).is_file());
    }

    let bed = TestBed::new();
    let mut cmd = storage_cmd(&bed);
    cmd.env("QF_STORAGE", "postgres")
        .env("QF_POSTGRES_URL", good_url())
        .args(STORE_STATUS);
    refused(&bed, &mut cmd, POSTGRES_REFUSAL);
}

#[test]
fn postgres_without_a_url_fails_naming_both_its_sources() {
    let bed = TestBed::new();
    let mut by_variable = storage_cmd(&bed);
    by_variable.env("QF_STORAGE", "postgres").args(STORE_STATUS);
    let mut by_flag = storage_cmd(&bed);
    by_flag.args(["--storage", "postgres"]).args(STORE_STATUS);
    for cmd in [&mut by_variable, &mut by_flag] {
        if cfg!(feature = "postgres") {
            refused(
                &bed,
                cmd,
                "postgres storage needs a connection URL: \
                 postgres://USER:PASSWORD@HOST:PORT/DATABASE; set QF_POSTGRES_URL, or pass \
                 --postgres-url",
            );
        } else {
            refused(&bed, cmd, POSTGRES_REFUSAL);
        }
    }
}

#[test]
fn an_unknown_backend_fails_listing_the_backends_before_anything_is_created() {
    let bed = TestBed::new();
    let mut by_flag = storage_cmd(&bed);
    by_flag.args(["--storage", "mysql"]).args(STORE_STATUS);
    let mut by_variable = storage_cmd(&bed);
    by_variable.env("QF_STORAGE", "mysql").args(STORE_STATUS);
    let mut empty = storage_cmd(&bed);
    empty.env("QF_STORAGE", "").args(STORE_STATUS);
    for cmd in [&mut by_flag, &mut by_variable, &mut empty] {
        let (code, text) = run(cmd);
        assert_eq!(code, Some(2), "{text}");
        assert!(
            text.contains("[possible values: sqlite, postgres]"),
            "{text}"
        );
    }
    assert_nothing_created(&bed);
}

#[cfg(feature = "postgres")]
#[test]
fn the_postgres_url_flag_beats_its_variable() {
    let bed = TestBed::new();
    let mut cmd = storage_cmd(&bed);
    cmd.args(["--storage", "postgres", "--postgres-url"])
        .arg(bad_url())
        .env("QF_POSTGRES_URL", good_url())
        .args(STORE_STATUS);
    refused(
        &bed,
        &mut cmd,
        "--postgres-url is not a usable PostgreSQL URL",
    );

    let mut cmd = storage_cmd(&bed);
    cmd.args(STORE_STATUS)
        .args(["--storage", "postgres", "--postgres-url"])
        .arg(good_url())
        .env("QF_POSTGRES_URL", bad_url());
    refused(&bed, &mut cmd, POSTGRES_REFUSAL);

    let mut cmd = storage_cmd(&bed);
    cmd.args(["--storage", "postgres"])
        .env("QF_POSTGRES_URL", bad_url())
        .args(STORE_STATUS);
    refused(
        &bed,
        &mut cmd,
        "QF_POSTGRES_URL is not a usable PostgreSQL URL: sslmode=disable would allow plain text",
    );
}

#[test]
fn a_postgres_setting_beside_an_omitted_backend_is_refused_naming_its_source() {
    let bed = TestBed::new();
    let cases: [(&str, &str, String); 4] = [
        ("QF_POSTGRES_URL", "", good_url()),
        ("QF_POSTGRES_POOL_SIZE", "", "4".to_string()),
        ("", "--postgres-pool-size", "4".to_string()),
        ("", "--postgres-connect-timeout-secs", "9".to_string()),
    ];
    for (variable, flag, value) in cases {
        let mut cmd = storage_cmd(&bed);
        let named = if variable.is_empty() {
            cmd.arg(flag).arg(&value);
            flag
        } else {
            cmd.env(variable, &value);
            variable
        };
        cmd.args(STORE_STATUS);
        refused(
            &bed,
            &mut cmd,
            &format!(
                "{named} is a PostgreSQL setting, but no storage backend is selected, so this \
                 would use SQLite"
            ),
        );
    }
}

#[test]
fn an_explicit_sqlite_backend_leaves_the_postgres_settings_unused() {
    let bed = TestBed::new();
    let (code, text) = run(storage_cmd(&bed)
        .args(["--storage", "sqlite", "--postgres-pool-size", "4"])
        .env("QF_POSTGRES_URL", bad_url())
        .args(STORE_STATUS));
    assert_eq!(code, Some(0), "{text}");
    assert!(bed.dir().join(DEFAULT_DB).is_file());
}

#[cfg(feature = "postgres")]
#[test]
fn a_sqlite_path_beside_postgres_is_refused_naming_its_source() {
    let bed = TestBed::new();
    let mut cmd = storage_cmd(&bed);
    cmd.args(["--storage", "postgres", "--db", "market.sqlite"])
        .env("QF_POSTGRES_URL", good_url())
        .args(STORE_STATUS);
    refused(
        &bed,
        &mut cmd,
        "--db names a SQLite file, but the postgres backend is selected",
    );

    let mut cmd = storage_cmd(&bed);
    cmd.env("QF_STORAGE", "postgres")
        .env("QF_DB", "market.sqlite")
        .env("QF_POSTGRES_URL", good_url())
        .args(STORE_STATUS);
    refused(
        &bed,
        &mut cmd,
        "QF_DB names a SQLite file, but the postgres backend is selected",
    );
}

#[test]
fn an_exported_but_empty_storage_variable_never_falls_back() {
    let bed = TestBed::new();
    for variable in ["QF_POSTGRES_POOL_SIZE", "QF_POSTGRES_CONNECT_TIMEOUT_SECS"] {
        let (code, text) = run(storage_cmd(&bed).env(variable, "").args(STORE_STATUS));
        assert_eq!(code, Some(2), "{variable}: {text}");
    }
    let mut cmd = storage_cmd(&bed);
    cmd.env("QF_POSTGRES_URL", "").args(STORE_STATUS);
    refused(&bed, &mut cmd, "QF_POSTGRES_URL is a PostgreSQL setting");
    #[cfg(feature = "postgres")]
    {
        let mut cmd = storage_cmd(&bed);
        cmd.env("QF_POSTGRES_URL", "")
            .args(["--storage", "postgres"])
            .args(STORE_STATUS);
        refused(
            &bed,
            &mut cmd,
            "QF_POSTGRES_URL is not a usable PostgreSQL URL",
        );
    }
    assert_nothing_created(&bed);
}

/// The commands that open a store refuse alike, `serve run` (which owns
/// a larger pool) included, before it binds.
#[test]
fn a_valid_postgres_selection_is_refused_and_never_falls_back_to_sqlite() {
    let bed = TestBed::new();
    let commands: [&[&str]; 3] = [
        &STORE_STATUS,
        &["data", "validate", "--symbol", "BTCUSDT"],
        &["serve", "run", "--listen", "127.0.0.1:0"],
    ];
    for command in commands {
        let mut cmd = storage_cmd(&bed);
        cmd.args(["--storage", "postgres"])
            .env("QF_POSTGRES_URL", good_url())
            .args(command);
        refused(&bed, &mut cmd, POSTGRES_REFUSAL);
    }
}

#[test]
fn the_help_names_the_url_variable_and_never_shows_its_value() {
    let bed = TestBed::new();
    for args in [&["--help"][..], &["store", "status", "--help"]] {
        let (code, text) = run(storage_cmd(&bed)
            .env("QF_POSTGRES_URL", good_url())
            .args(args));
        assert_eq!(code, Some(0), "{text}");
        assert!(text.contains("[env: QF_POSTGRES_URL]"), "{text}");
    }
}
