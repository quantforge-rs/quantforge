//! The PostgreSQL URL: ADR 0001's `sslmode` table, its parts, every
//! refusal, and a password that no `Display`, `Debug` or error shows.

use super::*;

/// libpq's `sslmode`s against ADR 0001's table, for loopback and other
/// hosts.
#[test]
fn sslmodes_follow_adr_0001s_table() {
    let loopback = ["localhost", "LOCALHOST", "127.0.0.1", "127.5.6.7", "[::1]"];
    let remote = [
        "db.example.com",
        "10.0.0.5",
        "[2001:db8::1]",
        "localhost.example",
    ];
    let mode = |host: &str, query: &str| {
        parse(&format!("postgres://u@{host}/db{query}")).map(|c| c.tls().mode)
    };
    for host in loopback {
        assert_eq!(mode(host, ""), Ok(TlsMode::Prefer), "{host}");
        assert_eq!(
            mode(host, "?sslmode=disable"),
            Ok(TlsMode::Disable),
            "{host}"
        );
        assert_eq!(mode(host, "?sslmode=prefer"), Ok(TlsMode::Prefer), "{host}");
        for strict in ["require", "verify-ca", "verify-full"] {
            assert_eq!(
                mode(host, &format!("?sslmode={strict}")),
                Ok(TlsMode::Require),
                "{host} {strict}"
            );
        }
    }
    for host in remote {
        assert_eq!(mode(host, ""), Ok(TlsMode::Require), "{host}");
        for plain in ["disable", "prefer"] {
            let refused = mode(host, &format!("?sslmode={plain}")).expect_err(host);
            assert!(
                matches!(refused, UrlProblem::PlainTextToRemoteHost { .. }),
                "{host} {plain}: {refused:?}"
            );
        }
        for strict in ["require", "verify-ca", "verify-full"] {
            assert_eq!(
                mode(host, &format!("?sslmode={strict}")),
                Ok(TlsMode::Require),
                "{host} {strict}"
            );
        }
    }
}

#[test]
fn the_url_reads_its_parts_with_defaults_and_decoding() {
    let connection =
        parse("postgres://quant%20bot:p%40ss@[::1]/my%2Fdb?sslmode=require").expect("connection");
    assert_eq!(connection.host(), "::1");
    assert_eq!(connection.port(), 5432);
    assert_eq!(connection.user(), "quant bot");
    assert!(connection.has_password());
    assert_eq!(connection.dbname(), "my/db");
    assert_eq!(connection.to_string(), "postgres://[::1]:5432/my/db");

    let without = parse("postgres://u@localhost:5433/db").expect("connection");
    assert!(!without.has_password());
    assert_eq!(without.to_string(), "postgres://localhost:5433/db");
}

/// Whether a refusal is the one a case expects.
type Expected = fn(&UrlProblem) -> bool;

#[test]
fn every_malformed_url_is_refused_with_its_problem() {
    let cases: &[(&str, Expected)] = &[
        ("", |p| matches!(p, UrlProblem::NotAUrl(_))),
        ("db.example.com/db", |p| matches!(p, UrlProblem::NotAUrl(_))),
        ("https://u@db.example.com/db", |p| {
            matches!(p, UrlProblem::Scheme)
        }),
        ("postgres://u@db.example.com/db#x", |p| {
            matches!(p, UrlProblem::Fragment)
        }),
        ("postgres:///db", |p| matches!(p, UrlProblem::NoHost)),
        ("postgres://u@a,b/db", |p| {
            matches!(p, UrlProblem::SeveralHosts)
        }),
        ("postgres://u@h1:5432,h2:5432/db", |p| {
            matches!(p, UrlProblem::NotAUrl(_))
        }),
        ("postgres://u@localhost:0/db", |p| {
            matches!(p, UrlProblem::PortZero)
        }),
        ("postgres://localhost/db", |p| {
            matches!(p, UrlProblem::NoUser)
        }),
        ("postgres://:pw@localhost/db", |p| {
            matches!(p, UrlProblem::NoUser)
        }),
        ("postgres://u@localhost", |p| {
            matches!(p, UrlProblem::NoDatabase)
        }),
        ("postgres://u@localhost/", |p| {
            matches!(p, UrlProblem::NoDatabase)
        }),
        ("postgres://u@localhost/a/b", |p| {
            matches!(p, UrlProblem::NoDatabase)
        }),
        ("postgres://u%FF@localhost/db", |p| {
            matches!(p, UrlProblem::Encoding)
        }),
        ("postgres://u@localhost/db?connect_timeout=9", |p| {
            *p == UrlProblem::Parameter {
                key: Some("connect_timeout".to_string()),
            }
        }),
        ("postgres://u@localhost/db?password=x", |p| {
            *p == UrlProblem::Parameter {
                key: Some("password".to_string()),
            }
        }),
        ("postgres://u@localhost/db?Hunter2", |p| {
            *p == UrlProblem::Parameter { key: None }
        }),
        (
            "postgres://u@localhost/db?sslmode=require&sslmode=require",
            |p| *p == UrlProblem::Repeated { key: "sslmode" },
        ),
        ("postgres://u@localhost/db?sslmode=allow", |p| {
            matches!(p, UrlProblem::SslMode)
        }),
        (
            "postgres://u@localhost/db?sslmode=disable&sslrootcert=ca.pem",
            |p| matches!(p, UrlProblem::CaFileWithoutTls),
        ),
        (
            "postgres://u@localhost/db?sslrootcert=no-such-ca.pem",
            |p| matches!(p, UrlProblem::CaFile { .. }),
        ),
    ];
    for (text, expected) in cases {
        let problem = parse(text).expect_err(text);
        assert!(expected(&problem), "{text}: {problem:?}");
    }
}

#[test]
fn a_readable_ca_file_is_kept() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, "not checked until a connection is made").expect("write");
    let text = format!(
        "postgres://u@db.example.com/db?sslmode=verify-full&sslrootcert={}",
        ca.display()
    );
    let connection = parse(&text).expect("connection");
    assert_eq!(connection.tls().ca_file.as_deref(), Some(ca.as_path()));
    assert_eq!(connection.tls().mode, TlsMode::Require);
    // A directory is no file.
    let text = format!(
        "postgres://u@db.example.com/db?sslrootcert={}",
        dir.path().display()
    );
    assert!(matches!(parse(&text), Err(UrlProblem::CaFile { .. })));
}

/// The password, and the URL that carries it, never appear in anything
/// these types show: no `Display`, no `Debug`, no error.
#[test]
fn no_display_debug_or_error_shows_the_password_or_the_url() {
    let full = format!("postgres://quant:{SENTINEL}@db.example.com:6432/market?sslmode=require");
    let mut shown = vec![
        format!("{:?}", url(&full)),
        format!("{:?}", Supplied::new(url(&full), "QF_POSTGRES_URL")),
        format!("{:?}", postgres_request(&full)),
    ];
    let connection = parse(&full).expect("connection");
    shown.push(connection.to_string());
    shown.push(format!("{connection:?}"));
    assert!(format!("{connection:?}").contains("<redacted>"));
    #[cfg(feature = "postgres")]
    {
        let settings = StorageSettings::resolve(postgres_request(&full), StoreOwner::Command)
            .expect("settings");
        shown.push(settings.to_string());
        shown.push(format!("{settings:?}"));
    }
    // Each refusal of a URL that carries the password.
    for bad in [
        format!("postgres://quant:{SENTINEL}@db.example.com/market?sslmode=disable"),
        format!("postgres://quant:{SENTINEL}@db.example.com/market?{SENTINEL}"),
        format!("postgres://quant:{SENTINEL}@db.example.com/market?sslmode={SENTINEL}"),
        format!("postgres://quant:{SENTINEL}@db.example.com"),
        format!("postgres://quant:{SENTINEL}@db.example.com:99999/market"),
        format!("ftp://quant:{SENTINEL}@db.example.com/market"),
    ] {
        let problem = parse(&bad).expect_err("refused");
        shown.push(problem.to_string());
        shown.push(format!("{problem:?}"));
        let error = StorageConfigError::InvalidPostgresUrl {
            label: "QF_POSTGRES_URL".to_string(),
            problem,
        };
        shown.push(error.to_string());
        shown.push(format!("{error:?}"));
    }
    for text in &shown {
        // Never print `text` itself: a failing run must not show the leak.
        assert!(!text.contains(SENTINEL), "the password leaked");
        assert!(!text.contains("quant:"), "the userinfo leaked");
    }
}

#[test]
fn a_connection_string_says_whether_it_carries_a_password() {
    assert!(url(&format!("postgres://u:{SENTINEL}@localhost/db")).has_password());
    assert!(!url("postgres://u@localhost/db").has_password());
    assert!(!url("not a url").has_password());
}
