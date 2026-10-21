//! libpq's `sslmode`s mapped onto what tokio-postgres knows, and plain text
//! refused for any host but loopback. No database needed.

use std::path::PathBuf;

use postgres_bridge_spike::tls::{TlsError, connector, prepare};
use tokio_postgres::config::SslMode;

fn mode(url: &str) -> SslMode {
    prepare(url).expect("a URL").config.get_ssl_mode()
}

#[test]
fn verify_full_and_verify_ca_become_require_with_their_ca_file() {
    for libpq in ["verify-full", "verify-ca"] {
        let prepared = prepare(&format!(
            "postgres://quantforge@db.example.com/market?sslmode={libpq}&sslrootcert=/etc/qf/ca.pem"
        ))
        .expect("a URL");
        assert_eq!(prepared.config.get_ssl_mode(), SslMode::Require, "{libpq}");
        assert_eq!(prepared.ca_file, Some(PathBuf::from("/etc/qf/ca.pem")));
    }
}

#[test]
fn a_remote_host_uses_tls_unless_told_otherwise() {
    assert_eq!(
        mode("postgres://quantforge@db.example.com/market"),
        SslMode::Require
    );
    assert_eq!(
        mode("postgres://quantforge@db.example.com/market?sslmode=require"),
        SslMode::Require
    );
}

#[test]
fn plain_text_to_a_remote_host_is_refused() {
    for plain in ["disable", "prefer"] {
        let refused = prepare(&format!(
            "postgres://quantforge@db.example.com/market?sslmode={plain}"
        ));
        assert!(
            matches!(&refused, Err(TlsError::PlainTextToRemoteHost { host, mode }) if host == "db.example.com" && mode == plain),
            "{refused:?}"
        );
    }
}

#[test]
fn loopback_may_skip_tls() {
    assert_eq!(
        mode("postgres://spike@127.0.0.1:5432/spike?sslmode=disable"),
        SslMode::Disable
    );
    assert_eq!(mode("postgres://spike@localhost/spike"), SslMode::Prefer);
    assert_eq!(
        mode("postgres://spike@[::1]/spike?sslmode=prefer"),
        SslMode::Prefer
    );
}

#[test]
fn an_unknown_mode_is_refused() {
    assert!(matches!(
        prepare("postgres://spike@localhost/spike?sslmode=verify-everything"),
        Err(TlsError::UnknownMode(_))
    ));
}

#[test]
fn a_ca_file_that_cannot_be_read_is_refused() {
    let missing = PathBuf::from("/nonexistent/qf-ca.pem");
    assert!(matches!(
        connector(Some(&missing)),
        Err(TlsError::CaFile { .. })
    ));
}
