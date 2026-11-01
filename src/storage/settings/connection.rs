//! A PostgreSQL connection URL, checked and read into its parts without the
//! driver, so that a build without the `postgres` feature checks it alike.
//!
//! The form is libpq's: `postgres://USER:PASSWORD@HOST:PORT/DATABASE`,
//! with `postgresql://` accepted too. One host, a user and a database are
//! required; the port defaults to 5432; user, password and database are
//! percent-decoded. Of libpq's query parameters only the two TLS ones are
//! read, and anything else is refused rather than silently dropped:
//!
//! - `sslmode`: tokio-postgres knows `disable`, `prefer` and `require`,
//!   and its rustls connector always checks the certificate chain and the
//!   host name, so `verify-ca` and `verify-full` become `require`. Plain
//!   text, or a fallback to it, is allowed only to this machine: `disable`
//!   and `prefer` are refused for any other host, which defaults to
//!   `require` (ADR 0001's table).
//! - `sslrootcert`: a CA file to trust beside the system's.
//!
//! The password stays inside [`PostgresConnection`]: its `Display` is
//! `postgres://HOST:PORT/DATABASE`, its `Debug` redacts the password, and no
//! [`UrlProblem`] quotes a value.

use percent_encoding::percent_decode_str;
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use url::{Host, Url};

const DEFAULT_PORT: u16 = 5432;

/// A PostgreSQL URL as a host supplied it, before it is checked. It may
/// carry a password, so it has no `Display` and no `Serialize`, and its
/// `Debug` shows none of it.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionString(String);

impl ConnectionString {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// Whether it carries a password, which an operator should keep out of
    /// shell history and process listings.
    pub fn has_password(&self) -> bool {
        Url::parse(&self.0).is_ok_and(|url| url.password().is_some())
    }
}

impl fmt::Debug for ConnectionString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConnectionString(<redacted>)")
    }
}

/// The TLS modes tokio-postgres knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Plain text; only to this machine.
    Disable,
    /// TLS when the server offers it, else plain text; only to this machine.
    Prefer,
    /// TLS with the certificate chain and host name checked.
    Require,
}

impl TlsMode {
    /// `disable`, `prefer` or `require`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Prefer => "prefer",
            Self::Require => "require",
        }
    }
}

/// How a connection uses TLS.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PostgresTls {
    pub mode: TlsMode,
    /// A CA to trust beside the system's, from `sslrootcert`.
    pub ca_file: Option<PathBuf>,
}

/// A checked PostgreSQL URL; see the module docs.
#[derive(Clone, PartialEq, Eq)]
pub struct PostgresConnection {
    host: String,
    port: u16,
    user: String,
    password: Option<Password>,
    dbname: String,
    tls: PostgresTls,
}

/// Never shown: its `Debug` is a placeholder and it has no `Display`.
#[derive(Clone, PartialEq, Eq)]
struct Password(String);

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl PostgresConnection {
    /// Checks `url` and reads it into its parts; see the module docs.
    pub fn parse(url: &ConnectionString) -> Result<Self, UrlProblem> {
        let url = Url::parse(&url.0).map_err(UrlProblem::NotAUrl)?;
        if !matches!(url.scheme(), "postgres" | "postgresql") {
            return Err(UrlProblem::Scheme);
        }
        if url.fragment().is_some() {
            return Err(UrlProblem::Fragment);
        }
        let (host, loopback) = match url.host() {
            None => return Err(UrlProblem::NoHost),
            Some(Host::Domain(name)) => {
                let name = decode(name)?;
                if name.is_empty() {
                    return Err(UrlProblem::NoHost);
                }
                if name.contains(',') {
                    return Err(UrlProblem::SeveralHosts);
                }
                let loopback = name.eq_ignore_ascii_case("localhost")
                    || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
                (name, loopback)
            }
            Some(Host::Ipv4(ip)) => (ip.to_string(), ip.is_loopback()),
            Some(Host::Ipv6(ip)) => (ip.to_string(), ip.is_loopback()),
        };
        let port = url.port().unwrap_or(DEFAULT_PORT);
        if port == 0 {
            return Err(UrlProblem::PortZero);
        }
        let user = decode(url.username())?;
        if user.is_empty() {
            return Err(UrlProblem::NoUser);
        }
        let password = url.password().map(decode).transpose()?.map(Password);
        let dbname = match url.path().strip_prefix('/') {
            Some(segment) if !segment.contains('/') => decode(segment)?,
            _ => return Err(UrlProblem::NoDatabase),
        };
        if dbname.is_empty() {
            return Err(UrlProblem::NoDatabase);
        }
        let tls = read_tls(&url, loopback)?;
        Ok(Self {
            host,
            port,
            user,
            password,
            dbname,
            tls,
        })
    }

    /// A host name or IP address, without brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn dbname(&self) -> &str {
        &self.dbname
    }

    pub fn tls(&self) -> &PostgresTls {
        &self.tls
    }

    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }

    /// The password, for the driver only.
    #[cfg(feature = "postgres")]
    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_ref().map(|password| password.0.as_str())
    }
}

/// `postgres://HOST:PORT/DATABASE`: no user, no password, no parameters.
impl fmt::Display for PostgresConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(
                f,
                "postgres://[{}]:{}/{}",
                self.host, self.port, self.dbname
            )
        } else {
            write!(f, "postgres://{}:{}/{}", self.host, self.port, self.dbname)
        }
    }
}

impl fmt::Debug for PostgresConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresConnection")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password)
            .field("dbname", &self.dbname)
            .field("tls", &self.tls)
            .finish()
    }
}

fn decode(raw: &str) -> Result<String, UrlProblem> {
    percent_decode_str(raw)
        .decode_utf8()
        .map(|text| text.into_owned())
        .map_err(|_| UrlProblem::Encoding)
}

/// The two TLS parameters, mapped as the module docs say; any other
/// parameter is refused.
fn read_tls(url: &Url, loopback: bool) -> Result<PostgresTls, UrlProblem> {
    let (mut mode, mut ca_file) = (None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "sslmode" if mode.is_some() => return Err(UrlProblem::Repeated { key: "sslmode" }),
            "sslmode" => mode = Some(value.into_owned()),
            "sslrootcert" if ca_file.is_some() => {
                return Err(UrlProblem::Repeated { key: "sslrootcert" });
            }
            "sslrootcert" => ca_file = Some(PathBuf::from(value.as_ref())),
            other => {
                return Err(UrlProblem::Parameter {
                    key: keyword(other),
                });
            }
        }
    }
    let mode = match mode.as_deref() {
        None if loopback => TlsMode::Prefer,
        None | Some("require" | "verify-ca" | "verify-full") => TlsMode::Require,
        Some("disable") if !loopback => {
            return Err(UrlProblem::PlainTextToRemoteHost { mode: "disable" });
        }
        Some("prefer") if !loopback => {
            return Err(UrlProblem::PlainTextToRemoteHost { mode: "prefer" });
        }
        Some("disable") => TlsMode::Disable,
        Some("prefer") => TlsMode::Prefer,
        Some(_) => return Err(UrlProblem::SslMode),
    };
    if let Some(path) = &ca_file {
        if mode == TlsMode::Disable {
            return Err(UrlProblem::CaFileWithoutTls);
        }
        if !is_readable_file(path) {
            return Err(UrlProblem::CaFile { path: path.clone() });
        }
    }
    Ok(PostgresTls { mode, ca_file })
}

fn is_readable_file(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(|file| file.metadata().is_ok_and(|meta| meta.is_file()))
}

/// A parameter's name as an error may quote it: a libpq keyword is lower
/// case letters and underscores, and anything else might be a value
/// written where a key belongs, so it is not repeated.
fn keyword(key: &str) -> Option<String> {
    let plain = !key.is_empty()
        && key.len() <= 32
        && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    plain.then(|| key.to_string())
}

/// What is wrong with a PostgreSQL URL. None of these quotes the URL or a
/// value from it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UrlProblem {
    NotAUrl(url::ParseError),
    Scheme,
    Fragment,
    NoHost,
    SeveralHosts,
    PortZero,
    NoUser,
    NoDatabase,
    Encoding,
    Parameter { key: Option<String> },
    Repeated { key: &'static str },
    SslMode,
    PlainTextToRemoteHost { mode: &'static str },
    CaFileWithoutTls,
    CaFile { path: PathBuf },
}

impl fmt::Display for UrlProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const FORM: &str = "postgres://USER:PASSWORD@HOST:PORT/DATABASE";
        match self {
            Self::NotAUrl(error) => write!(f, "it does not parse as a URL ({error}); write {FORM}"),
            Self::Scheme => write!(f, "its scheme must be postgres:// or postgresql://"),
            Self::Fragment => f.write_str("it has a #fragment, which a PostgreSQL URL never has"),
            Self::NoHost => write!(f, "it names no host; write {FORM}"),
            Self::SeveralHosts => f.write_str("it names several hosts; QuantForge connects to one"),
            Self::PortZero => f.write_str("its port is 0"),
            Self::NoUser => write!(f, "it names no user; write {FORM}"),
            Self::NoDatabase => write!(f, "it names no single database; write {FORM}"),
            Self::Encoding => {
                f.write_str("its host, user, password or database is not percent-encoded UTF-8")
            }
            Self::Parameter { key: Some(key) } => write!(
                f,
                "it has the parameter {key}, which QuantForge does not read; only sslmode and \
                 sslrootcert are"
            ),
            Self::Parameter { key: None } => f.write_str(
                "it has a parameter QuantForge does not read; only sslmode and sslrootcert are",
            ),
            Self::Repeated { key } => write!(f, "it sets {key} more than once"),
            Self::SslMode => f.write_str(
                "its sslmode is not one of disable, prefer, require, verify-ca or verify-full",
            ),
            Self::PlainTextToRemoteHost { mode } => write!(
                f,
                "sslmode={mode} would allow plain text to a host other than this machine; only \
                 loopback may skip TLS, so use sslmode=verify-full (or require)"
            ),
            Self::CaFileWithoutTls => {
                f.write_str("it sets sslrootcert with sslmode=disable, which never uses it")
            }
            Self::CaFile { path } => write!(
                f,
                "its sslrootcert names no readable file: {}",
                path.display()
            ),
        }
    }
}
