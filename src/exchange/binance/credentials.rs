//! Binance API credentials and where they come from. [`BinanceCredentials::from_parts`]
//! is the resolution seam: it owns the both-or-nothing rule, so another source
//! (CLI arguments) can be added beside `from_env` without touching the client.

use std::env;

use crate::ExchangeError;

#[derive(Clone, Debug)]
pub struct BinanceCredentials {
    pub api_key: String,
    pub secret: String,
}

impl BinanceCredentials {
    /// Reads `QF_BINANCE_API_KEY` and `QF_BINANCE_API_SECRET` from the process
    /// environment.
    pub fn from_env() -> Option<Self> {
        Self::from_parts(
            env::var("QF_BINANCE_API_KEY").ok(),
            env::var("QF_BINANCE_API_SECRET").ok(),
        )
    }

    pub fn from_required_env() -> Result<Self, ExchangeError> {
        match Self::from_env() {
            Some(value) => Ok(value),
            None => Err(ExchangeError::MissingCredentials),
        }
    }

    /// Both-or-nothing: a half-configured pair is no credentials at all, so a
    /// missing secret can never silently pair with a leftover key and send a
    /// request that fails only at the exchange.
    fn from_parts(api_key: Option<String>, secret: Option<String>) -> Option<Self> {
        Some(Self {
            api_key: api_key?,
            secret: secret?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_parts_takes_a_complete_pair() {
        let credentials =
            BinanceCredentials::from_parts(Some("key".to_string()), Some("secret".to_string()))
                .expect("credentials");

        assert_eq!(credentials.api_key, "key");
        assert_eq!(credentials.secret, "secret");
    }

    #[test]
    fn from_parts_rejects_a_half_configured_pair() {
        assert!(BinanceCredentials::from_parts(Some("key".to_string()), None).is_none());
        assert!(BinanceCredentials::from_parts(None, Some("secret".to_string())).is_none());
        assert!(BinanceCredentials::from_parts(None, None).is_none());
    }
}
