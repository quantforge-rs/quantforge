//! An exchange API key and secret. [`ApiCredentials::resolve`] owns the
//! both-or-nothing rule; where the two values come from is the caller's
//! business. The CLI takes them from `--api-key`/`--api-secret` with
//! `QF_API_KEY`/`QF_API_SECRET` as env fallbacks, so the library never reads
//! credentials from the process environment.

use std::fmt;

#[derive(Clone)]
pub struct ApiCredentials {
    pub api_key: String,
    pub secret: String,
}

/// Prints neither the key nor the secret, so the clients, connectors, and
/// contexts that derive `Debug` around these credentials cannot leak them
/// into logs.
impl fmt::Debug for ApiCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiCredentials").finish_non_exhaustive()
    }
}

impl ApiCredentials {
    /// Both-or-nothing: a half-configured pair is no credentials at all, so a
    /// missing secret can never silently pair with a leftover key and send a
    /// request that fails only at the exchange. A blank value (empty or
    /// whitespace-only, such as an exported `QF_API_KEY=`) counts as
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
    use crate::ExchangeId;
    use crate::exchange::{ConnectorConfig, connect};

    #[test]
    fn resolve_takes_a_complete_pair() {
        let credentials =
            ApiCredentials::resolve(Some("key".to_string()), Some("secret".to_string()))
                .expect("credentials");

        assert_eq!(credentials.api_key, "key");
        assert_eq!(credentials.secret, "secret");
    }

    #[test]
    fn resolve_rejects_a_half_configured_pair() {
        assert!(ApiCredentials::resolve(Some("key".to_string()), None).is_none());
        assert!(ApiCredentials::resolve(None, Some("secret".to_string())).is_none());
        assert!(ApiCredentials::resolve(None, None).is_none());
    }

    // clap hands over an exported-but-empty env var as `Some("")`; it must
    // read as missing, not as a key the exchange would reject.
    #[test]
    fn resolve_treats_blank_values_as_missing() {
        for blank in ["", "  ", "\t\n"] {
            assert!(
                ApiCredentials::resolve(Some(blank.to_string()), Some("secret".to_string()))
                    .is_none(),
                "blank key {blank:?}"
            );
            assert!(
                ApiCredentials::resolve(Some("key".to_string()), Some(blank.to_string())).is_none(),
                "blank secret {blank:?}"
            );
        }
    }

    #[test]
    fn debug_output_never_contains_the_key_or_secret() {
        let credentials = ApiCredentials::resolve(
            Some("key-sentinel".to_string()),
            Some("secret-sentinel".to_string()),
        )
        .expect("credentials");
        let base_url = url::Url::parse("https://api.binance.com/").expect("url");
        let client = crate::exchange::binance::BinanceSpotClient::new(base_url.clone())
            .with_credentials(credentials.clone());
        let config = ConnectorConfig {
            base_url,
            credentials: Some(credentials.clone()),
        };
        let connectors = connect(ExchangeId::BinanceSpot, config.clone()).expect("connectors");
        assert!(connectors.signed().is_some(), "credentials were supplied");
        assert_eq!(
            connectors.market_data.exchange_id(),
            ExchangeId::BinanceSpot
        );

        // Failure messages name the owner, never the output: a failing run
        // must not print what leaked.
        for (owner, debug) in [
            ("credentials", format!("{credentials:?}")),
            ("client", format!("{client:?}")),
            ("config", format!("{config:?}")),
            ("connectors", format!("{connectors:?}")),
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
