//! Binance API credentials. [`BinanceCredentials::resolve`] owns the
//! both-or-nothing rule; where the two values come from is the caller's
//! business. The CLI takes them from `--binance-api-key`/`--binance-api-secret`
//! with `QF_BINANCE_API_KEY`/`QF_BINANCE_API_SECRET` as env fallbacks, so the
//! library itself never reads the process environment.

use std::fmt;

#[derive(Clone)]
pub struct BinanceCredentials {
    pub api_key: String,
    pub secret: String,
}

/// Prints neither the key nor the secret, so the clients and contexts that
/// derive `Debug` around these credentials cannot leak them into logs.
impl fmt::Debug for BinanceCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BinanceCredentials").finish_non_exhaustive()
    }
}

impl BinanceCredentials {
    /// Both-or-nothing: a half-configured pair is no credentials at all, so a
    /// missing secret can never silently pair with a leftover key and send a
    /// request that fails only at the exchange. A blank value (empty or
    /// whitespace-only, such as an exported `QF_BINANCE_API_KEY=`) counts as
    /// missing for the same reason; any other value is used verbatim.
    pub fn resolve(api_key: Option<String>, secret: Option<String>) -> Option<Self> {
        let present = |value: Option<String>| value.filter(|value| !value.trim().is_empty());
        Some(Self {
            api_key: present(api_key)?,
            secret: present(secret)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_takes_a_complete_pair() {
        let credentials =
            BinanceCredentials::resolve(Some("key".to_string()), Some("secret".to_string()))
                .expect("credentials");

        assert_eq!(credentials.api_key, "key");
        assert_eq!(credentials.secret, "secret");
    }

    #[test]
    fn resolve_rejects_a_half_configured_pair() {
        assert!(BinanceCredentials::resolve(Some("key".to_string()), None).is_none());
        assert!(BinanceCredentials::resolve(None, Some("secret".to_string())).is_none());
        assert!(BinanceCredentials::resolve(None, None).is_none());
    }

    // clap hands over an exported-but-empty env var as `Some("")`; it must
    // read as missing, not as a key the exchange would reject.
    #[test]
    fn resolve_treats_blank_values_as_missing() {
        for blank in ["", "  ", "\t\n"] {
            assert!(
                BinanceCredentials::resolve(Some(blank.to_string()), Some("secret".to_string()))
                    .is_none(),
                "blank key {blank:?}"
            );
            assert!(
                BinanceCredentials::resolve(Some("key".to_string()), Some(blank.to_string()))
                    .is_none(),
                "blank secret {blank:?}"
            );
        }
    }

    #[test]
    fn debug_output_never_contains_the_key_or_secret() {
        let credentials = BinanceCredentials::resolve(
            Some("key-sentinel".to_string()),
            Some("secret-sentinel".to_string()),
        )
        .expect("credentials");
        let client = crate::BinanceSpotClient::new(
            url::Url::parse("https://api.binance.com/").expect("url"),
        )
        .with_credentials(credentials.clone());

        // Failure messages name the owner, never the output: a failing run
        // must not print what leaked.
        for (owner, debug) in [
            ("credentials", format!("{credentials:?}")),
            ("client", format!("{client:?}")),
        ] {
            assert!(
                !debug.contains("key-sentinel"),
                "{owner} Debug output contains the key"
            );
            assert!(
                !debug.contains("secret-sentinel"),
                "{owner} Debug output contains the secret"
            );
        }
    }
}
