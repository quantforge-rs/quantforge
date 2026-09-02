//! HMAC-SHA256 query signing for Binance's signed endpoints. The functions
//! here are pure — the caller supplies the secret and the timestamp — so the
//! signature is reproducible and can be checked against fixed vectors.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use url::form_urlencoded;

use crate::{ExchangeError, TimestampMs};

type HmacSha256 = Hmac<Sha256>;

/// Form-encodes `params` in the order given.
pub(super) fn encode_query(params: Vec<(&str, String)>) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        serializer.append_pair(key, &value);
    }
    serializer.finish()
}

/// The hex-encoded HMAC-SHA256 of `query` under `secret`.
pub(super) fn sign(secret: &str, query: &str) -> Result<String, ExchangeError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|err| {
        ExchangeError::InvalidRequest {
            message: format!("invalid HMAC secret: {err}"),
        }
    })?;
    mac.update(query.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// The complete query string for a signed endpoint.
///
/// `recvWindow` then `timestamp` are appended last, the signature is computed
/// over the exact encoded string that goes on the wire, and `signature` is
/// appended after it. Binance rejects any other order, and re-serializing the
/// query after signing would invalidate the digest.
pub(super) fn signed_query(
    secret: &str,
    params: Vec<(&str, String)>,
    recv_window_ms: u64,
    timestamp_ms: TimestampMs,
) -> Result<String, ExchangeError> {
    let mut params = params;
    params.push(("recvWindow", recv_window_ms.to_string()));
    params.push(("timestamp", timestamp_ms.to_string()));

    let query = encode_query(params);
    let signature = sign(secret, &query)?;

    Ok(format!("{query}&signature={signature}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The worked example from Binance's own signed-endpoint documentation.
    const DOC_SECRET: &str = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    const DOC_QUERY: &str = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1\
                             &price=0.1&recvWindow=5000&timestamp=1499827319559";
    const DOC_SIGNATURE: &str = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71";

    #[test]
    fn encode_query_works() {
        assert_eq!(
            encode_query(vec![
                ("symbol", "BTCUSDT".to_string()),
                ("limit", "10".to_string())
            ]),
            "symbol=BTCUSDT&limit=10"
        );
    }

    // RFC 4231 HMAC-SHA256 test case 2: pins the primitive itself, independent
    // of any Binance-shaped input.
    #[test]
    fn sign_matches_rfc_4231_case_2() {
        assert_eq!(
            sign("Jefe", "what do ya want for nothing?").expect("signature"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn sign_matches_the_binance_documented_example() {
        assert_eq!(
            sign(DOC_SECRET, DOC_QUERY).expect("signature"),
            DOC_SIGNATURE
        );
    }

    // The whole contract in one assertion: parameter order preserved,
    // recvWindow then timestamp appended last, signature appended after.
    #[test]
    fn signed_query_reproduces_the_binance_documented_example() {
        let query = signed_query(
            DOC_SECRET,
            vec![
                ("symbol", "LTCBTC".to_string()),
                ("side", "BUY".to_string()),
                ("type", "LIMIT".to_string()),
                ("timeInForce", "GTC".to_string()),
                ("quantity", "1".to_string()),
                ("price", "0.1".to_string()),
            ],
            5_000,
            1_499_827_319_559,
        )
        .expect("signed query");

        assert_eq!(query, format!("{DOC_QUERY}&signature={DOC_SIGNATURE}"));
    }

    // The digest must cover the percent-encoded bytes actually sent, not the
    // raw parameter values: signing the decoded form would be rejected by the
    // exchange for any value carrying a reserved character.
    #[test]
    fn signed_query_signs_the_encoded_string_it_sends() {
        let query = signed_query(
            DOC_SECRET,
            vec![("newClientOrderId", "qf bot/1".to_string())],
            5_000,
            1,
        )
        .expect("signed query");

        let (sent, signature) = query.rsplit_once("&signature=").expect("signature suffix");
        assert_eq!(
            sent,
            "newClientOrderId=qf+bot%2F1&recvWindow=5000&timestamp=1"
        );
        assert_eq!(signature, sign(DOC_SECRET, sent).expect("signature"));
    }
}
