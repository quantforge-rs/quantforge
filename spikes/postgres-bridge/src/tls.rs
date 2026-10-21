//! TLS for PostgreSQL connections: rustls with the `ring` provider, the same
//! TLS stack reqwest already uses, so no OpenSSL anywhere.
//!
//! tokio-postgres understands three `sslmode`s, `disable`, `prefer`, and
//! `require`, and none of libpq's certificate options. [`prepare`] reads a
//! libpq-style URL into what it understands: `verify-ca` and `verify-full`
//! become `require`, because the rustls connector always verifies the
//! certificate chain and the host name, and `sslrootcert` becomes a CA file
//! the connector trusts besides the system's. A host other than loopback must
//! use TLS: `disable` and `prefer`, which falls back to plain text, are
//! refused for it, and it defaults to `require`.

use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres::config::{Host, SslMode};
use tokio_postgres_rustls::MakeRustlsConnect;
use url::Url;

/// Why a connection string or a CA file was refused.
#[derive(Debug)]
pub enum TlsError {
    /// Not a `postgres://` or `postgresql://` URL tokio-postgres accepts.
    InvalidUrl(String),
    /// An `sslmode` libpq does not know.
    UnknownMode(String),
    /// Plain text, or a fallback to it, to a host other than loopback.
    PlainTextToRemoteHost { host: String, mode: String },
    /// The CA file could not be read as PEM certificates.
    CaFile { path: PathBuf, reason: String },
    /// rustls refused the configuration.
    Config(rustls::Error),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(reason) => write!(f, "invalid PostgreSQL URL: {reason}"),
            Self::UnknownMode(mode) => write!(
                f,
                "unknown sslmode {mode:?}; use disable, prefer, require, verify-ca, or verify-full"
            ),
            Self::PlainTextToRemoteHost { host, mode } => write!(
                f,
                "sslmode={mode} would allow plain text to {host}; only loopback may skip TLS, so \
                 use sslmode=verify-full (or require)"
            ),
            Self::CaFile { path, reason } => {
                write!(f, "cannot read CA file {}: {reason}", path.display())
            }
            Self::Config(error) => write!(f, "TLS configuration refused: {error}"),
        }
    }
}

impl std::error::Error for TlsError {}

/// A connection string read into what tokio-postgres takes.
#[derive(Debug)]
pub struct Prepared {
    pub config: tokio_postgres::Config,
    /// A CA to trust besides the system's, from `sslrootcert`.
    pub ca_file: Option<PathBuf>,
}

/// Reads a libpq-style `postgres://` URL. See the module docs for the rules.
pub fn prepare(url: &str) -> Result<Prepared, TlsError> {
    let mut url = Url::parse(url).map_err(|error| TlsError::InvalidUrl(error.to_string()))?;
    let mut mode = None;
    let mut ca_file = None;
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter_map(|(key, value)| match key.as_ref() {
            "sslmode" => {
                mode = Some(value.into_owned());
                None
            }
            "sslrootcert" => {
                ca_file = Some(PathBuf::from(value.as_ref()));
                None
            }
            _ => Some((key.into_owned(), value.into_owned())),
        })
        .collect();
    url.query_pairs_mut().clear().extend_pairs(kept);
    if url.query() == Some("") {
        url.set_query(None);
    }

    let mut config = tokio_postgres::Config::from_str(url.as_str())
        .map_err(|error| TlsError::InvalidUrl(error.to_string()))?;
    let loopback = config.get_hosts().iter().all(is_loopback);
    let ssl_mode = match mode.as_deref() {
        None if loopback => SslMode::Prefer,
        None | Some("require" | "verify-ca" | "verify-full") => SslMode::Require,
        Some(plain @ ("disable" | "prefer")) if !loopback => {
            return Err(TlsError::PlainTextToRemoteHost {
                host: config
                    .get_hosts()
                    .iter()
                    .map(describe)
                    .collect::<Vec<_>>()
                    .join(","),
                mode: plain.to_string(),
            });
        }
        Some("disable") => SslMode::Disable,
        Some("prefer") => SslMode::Prefer,
        Some(other) => return Err(TlsError::UnknownMode(other.to_string())),
    };
    config.ssl_mode(ssl_mode);
    Ok(Prepared { config, ca_file })
}

fn is_loopback(host: &Host) -> bool {
    match host {
        Host::Tcp(name) => {
            name == "localhost" || IpAddr::from_str(name).is_ok_and(|ip| ip.is_loopback())
        }
        #[cfg(unix)]
        Host::Unix(_) => true,
    }
}

fn describe(host: &Host) -> String {
    match host {
        Host::Tcp(name) => name.clone(),
        #[cfg(unix)]
        Host::Unix(path) => path.display().to_string(),
    }
}

/// The connector: the system's trust store (`SSL_CERT_FILE` and
/// `SSL_CERT_DIR` replace it, as for any rustls program) plus `ca_file`. The
/// `ring` provider is passed in, never installed process-wide.
pub fn connector(ca_file: Option<&PathBuf>) -> Result<MakeRustlsConnect, TlsError> {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    if let Some(path) = ca_file {
        let refused = |reason: String| TlsError::CaFile {
            path: path.clone(),
            reason,
        };
        let certs = CertificateDer::pem_file_iter(path)
            .map_err(|error| refused(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| refused(error.to_string()))?;
        if certs.is_empty() {
            return Err(refused("no certificates in it".to_string()));
        }
        for cert in certs {
            roots
                .add(cert)
                .map_err(|error| refused(error.to_string()))?;
        }
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(TlsError::Config)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}
