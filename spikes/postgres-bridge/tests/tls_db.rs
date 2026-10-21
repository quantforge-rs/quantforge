//! TLS against a real server: verify-full with the cluster's own CA connects,
//! and without it the certificate is refused.

mod common;

use postgres_bridge_spike::bridge::{BridgeConfig, Effect, PgBridge};

use common::unique;

/// The TLS URL: host `localhost`, `sslmode=verify-full`, `sslrootcert`.
fn tls_url_or_skip() -> Option<String> {
    let url = std::env::var("SPIKE_TLS_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    if url.is_none() {
        eprintln!("skipping: SPIKE_TLS_DATABASE_URL is unset");
    }
    url
}

#[test]
fn verify_full_with_the_clusters_ca_connects_over_tls() {
    let Some(url) = tls_url_or_skip() else { return };
    let bridge =
        PgBridge::connect(&BridgeConfig::new(&url, unique("tls"))).expect("a TLS connection");
    let ssl = bridge
        .execute(Effect::Read, |client| {
            Box::pin(async move {
                let row = client
                    .query_typed_one(
                        "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
                        &[],
                    )
                    .await?;
                row.try_get::<_, bool>(0)
            })
        })
        .expect("the session's TLS state");
    assert!(ssl);
}

#[test]
fn without_the_ca_the_servers_certificate_is_refused() {
    let Some(url) = tls_url_or_skip() else { return };
    let without_ca = url
        .split('&')
        .filter(|part| !part.starts_with("sslrootcert="))
        .collect::<Vec<_>>()
        .join("&");
    let refused = PgBridge::connect(&BridgeConfig::new(&without_ca, unique("tls_no_ca")))
        .expect_err("an untrusted certificate");
    assert_eq!(refused.kind(), "unavailable", "{refused}");
}
