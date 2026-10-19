//! Configuration precedence for the global settings: a flag beats its
//! `QF_*` variable on either side of the subcommand, the variable beats the
//! built-in default, and an exported-but-empty variable never falls back to
//! that default, as `docs/configuration.md` states.
//!
//! Every command starts from [`unpinned_cmd`]: a cleared environment with
//! the bed as working directory, so the setting under test has no source
//! but the ones a case supplies, and even the default `data/market.sqlite`
//! lands inside the bed. The other settings are pinned by flag.
//!
//! The credential settings are asserted on the wire in `offline`, where the
//! mock names the key and secret that signed each request:
//! `credential_flags_override_env_vars_on_either_side_of_the_subcommand`,
//! `a_credential_flag_overrides_only_its_own_env_var`, and
//! `missing_credentials_name_both_the_flags_and_the_env_vars`.

use crate::harness::{TestBed, unpinned_cmd};
use assert_cmd::Command;
use predicates::prelude::*;
use std::ffi::OsString;

const DATA_VALIDATE: [&str; 6] = [
    "data",
    "validate",
    "--symbol",
    "BTCUSDT",
    "--interval",
    "1m",
];
const UNROUTABLE_URL: &str = "http://127.0.0.1:9/";

// --db: the database file a run creates names the path that won.

const FLAG_DB: &str = "from-flag.sqlite";
const ENV_DB: &str = "from-env.sqlite";
const DEFAULT_DB: &str = "data/market.sqlite";

/// Every setting but `--db` pinned.
fn db_cmd(bed: &TestBed) -> Command {
    let mut cmd = unpinned_cmd(bed);
    cmd.args(["--log-level", "error", "--binance-base-url", UNROUTABLE_URL]);
    cmd
}

/// Which candidate database files exist in the bed after a run.
fn created_databases(bed: &TestBed) -> Vec<&'static str> {
    [FLAG_DB, ENV_DB, DEFAULT_DB]
        .into_iter()
        .filter(|name| bed.dir().join(name).exists())
        .collect()
}

#[test]
fn db_flag_beats_qf_db_and_qf_db_beats_the_default() {
    for flag_first in [true, false] {
        let bed = TestBed::new();
        let flag: [OsString; 2] = ["--db".into(), bed.dir().join(FLAG_DB).into()];
        let mut cmd = db_cmd(&bed);
        cmd.env("QF_DB", bed.dir().join(ENV_DB));
        if flag_first {
            cmd.args(&flag).args(DATA_VALIDATE);
        } else {
            cmd.args(DATA_VALIDATE).args(&flag);
        }
        cmd.assert().success();
        assert_eq!(
            created_databases(&bed),
            [FLAG_DB],
            "flag and QF_DB, flag first: {flag_first}"
        );
    }

    let bed = TestBed::new();
    let mut cmd = db_cmd(&bed);
    cmd.env("QF_DB", bed.dir().join(ENV_DB)).args(DATA_VALIDATE);
    cmd.assert().success();
    assert_eq!(created_databases(&bed), [ENV_DB], "QF_DB alone");

    let bed = TestBed::new();
    let mut cmd = db_cmd(&bed);
    cmd.args(DATA_VALIDATE);
    cmd.assert().success();
    assert_eq!(created_databases(&bed), [DEFAULT_DB], "neither");
}

// --binance-base-url: `trade run --mode live` without `--yes` on piped
// stdio prints where REAL orders would go, then exits before the
// credential check and before any request.

const LIVE_PREVIEW: [&str; 6] = ["trade", "run", "--symbol", "BTCUSDT", "--mode", "live"];
const FLAG_URL: &str = "http://127.0.0.1:9/from-flag/";
const ENV_URL: &str = "http://127.0.0.1:9/from-env/";

/// Every setting but `--binance-base-url` pinned.
fn base_url_cmd(bed: &TestBed) -> Command {
    let mut cmd = unpinned_cmd(bed);
    cmd.arg("--db")
        .arg(bed.db_path())
        .args(["--log-level", "error"]);
    cmd
}

/// The endpoint the live preview names, production marker included.
fn previewed_endpoint(cmd: &mut Command) -> String {
    let assert = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("with REAL orders via "))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no live preview in stdout:\n{stdout}"))
}

#[test]
fn binance_base_url_flag_beats_its_env_var_and_the_env_var_beats_the_default() {
    for flag_first in [true, false] {
        let bed = TestBed::new();
        let flag = ["--binance-base-url", FLAG_URL];
        let mut cmd = base_url_cmd(&bed);
        cmd.env("QF_BINANCE_BASE_URL", ENV_URL);
        if flag_first {
            cmd.args(flag).args(LIVE_PREVIEW);
        } else {
            cmd.args(LIVE_PREVIEW).args(flag);
        }
        assert_eq!(
            previewed_endpoint(&mut cmd),
            FLAG_URL,
            "flag and QF_BINANCE_BASE_URL, flag first: {flag_first}"
        );
    }

    let bed = TestBed::new();
    let mut cmd = base_url_cmd(&bed);
    cmd.env("QF_BINANCE_BASE_URL", ENV_URL).args(LIVE_PREVIEW);
    assert_eq!(
        previewed_endpoint(&mut cmd),
        ENV_URL,
        "QF_BINANCE_BASE_URL alone"
    );

    // The default is production, so no unroutable URL can back this case
    // up; the cleared environment does instead: without credentials, a
    // run that skipped the preview would stop at the credential gate.
    let bed = TestBed::new();
    let mut cmd = base_url_cmd(&bed);
    cmd.args(LIVE_PREVIEW);
    assert_eq!(
        previewed_endpoint(&mut cmd),
        "https://api.binance.com/ (PRODUCTION)",
        "neither"
    );
}

// --log-level: the INFO line every command logs at startup (on stdout)
// shows whether the info level is on.

const STARTUP_INFO: &str = "using Binance base URL";

/// Every setting but `--log-level` pinned.
fn log_level_cmd(bed: &TestBed) -> Command {
    let mut cmd = unpinned_cmd(bed);
    cmd.arg("--db")
        .arg(bed.db_path())
        .args(["--binance-base-url", UNROUTABLE_URL]);
    cmd
}

fn logs_startup_info(cmd: &mut Command) -> bool {
    let assert = cmd.assert().success();
    String::from_utf8_lossy(&assert.get_output().stdout).contains(STARTUP_INFO)
}

#[test]
fn log_level_flag_beats_qf_log_and_qf_log_beats_the_default() {
    // The flag wins in both directions, so precedence is not "the most
    // restrictive filter wins".
    for (flag_level, env_level, logs_info) in [("error", "info", false), ("info", "error", true)] {
        for flag_first in [true, false] {
            let bed = TestBed::new();
            let flag = ["--log-level", flag_level];
            let mut cmd = log_level_cmd(&bed);
            cmd.env("QF_LOG", env_level);
            if flag_first {
                cmd.args(flag).args(DATA_VALIDATE);
            } else {
                cmd.args(DATA_VALIDATE).args(flag);
            }
            assert_eq!(
                logs_startup_info(&mut cmd),
                logs_info,
                "--log-level {flag_level} with QF_LOG={env_level}, flag first: {flag_first}"
            );
        }
    }

    let bed = TestBed::new();
    let mut cmd = log_level_cmd(&bed);
    cmd.env("QF_LOG", "error").args(DATA_VALIDATE);
    assert!(
        !logs_startup_info(&mut cmd),
        "QF_LOG=error alone still logged at info"
    );

    let bed = TestBed::new();
    let mut cmd = log_level_cmd(&bed);
    cmd.args(DATA_VALIDATE);
    assert!(
        logs_startup_info(&mut cmd),
        "the default level does not log at info"
    );
}

// The filter is built with `EnvFilter::try_new`, never from `RUST_LOG`;
// this pins that against a switch to `EnvFilter::from_default_env`.
#[test]
fn rust_log_never_sets_the_log_filter() {
    let bed = TestBed::new();
    let mut cmd = log_level_cmd(&bed);
    cmd.env("RUST_LOG", "error").args(DATA_VALIDATE);
    assert!(
        logs_startup_info(&mut cmd),
        "RUST_LOG=error overrode the default info level"
    );
}

#[test]
fn an_exported_but_empty_variable_never_falls_back_to_the_default() {
    // clap rejects the empty path before anything is opened or created.
    let bed = TestBed::new();
    let mut cmd = db_cmd(&bed);
    cmd.env("QF_DB", "").args(DATA_VALIDATE);
    cmd.assert().code(2).stderr(predicate::str::contains(
        "a value is required for '--db <DB>'",
    ));
    assert!(
        created_databases(&bed).is_empty(),
        "an empty QF_DB created a database"
    );

    // An empty URL fails to parse; it never becomes the production default.
    let bed = TestBed::new();
    let mut cmd = base_url_cmd(&bed);
    cmd.env("QF_BINANCE_BASE_URL", "").args(LIVE_PREVIEW);
    cmd.assert()
        .code(1)
        .stdout(predicate::str::contains("api.binance.com").not())
        .stderr(predicate::str::contains(
            "failed to parse --binance-base-url",
        ));

    // An empty filter turns logging off rather than restoring info.
    let bed = TestBed::new();
    let mut cmd = log_level_cmd(&bed);
    cmd.env("QF_LOG", "").args(DATA_VALIDATE);
    assert!(
        !logs_startup_info(&mut cmd),
        "an empty QF_LOG fell back to the default info level"
    );
}
