//! A server's URL: checked as `--remote` takes it, shown without the
//! credentials it can carry, and whether it names this machine.

use url::Url;

use super::ClientError;

/// A server's URL, checked: `http` or `https`, a host, no query or
/// fragment, and a path that ends with `/`, so that the routes join below
/// it, as behind a proxy that serves the server under a path.
///
/// ```
/// use quantforge::net::client::server_url;
///
/// let url = server_url("http://127.0.0.1:9600")?;
/// assert_eq!(url.as_str(), "http://127.0.0.1:9600/");
/// assert_eq!(server_url("https://qf.example/lab")?.as_str(), "https://qf.example/lab/");
/// assert!(server_url("ftp://qf.example").is_err());
/// # Ok::<(), quantforge::net::client::ClientError>(())
/// ```
pub fn server_url(text: &str) -> Result<Url, ClientError> {
    let invalid = |reason: &str| ClientError::InvalidUrl {
        reason: reason.to_string(),
    };
    let mut url = Url::parse(text.trim()).map_err(|error| invalid(&error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("a server's URL starts with http:// or https://"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid("a server's URL names a host"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("a server's URL has no query or fragment"));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

/// The URL as it may be shown: without the user name and password it can
/// carry.
pub(super) fn shown(url: &Url) -> String {
    let mut shown = url.clone();
    let _ = shown.set_username("");
    let _ = shown.set_password(None);
    shown.to_string()
}

/// Whether `url` names this machine.
pub(super) fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}
