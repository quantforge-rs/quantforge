//! Binance's error codes and HTTP statuses, classified as the
//! exchange-neutral kinds callers branch on. A few codes carry several
//! distinct refusals — -2010, -2011 and -1013 among them — and for those
//! the message decides.

use reqwest::StatusCode;

use crate::ExchangeErrorKind;

/// The kind of a failed response. `code` and `message` are Binance's error
/// body when it sent one, and `message` is the raw body otherwise.
///
/// The status decides first when it is a rate limit, since Binance answers
/// 429 and 418 before looking at the request; then a code Binance
/// documents; then the status class; anything left is `Other`.
pub(super) fn classify(status: StatusCode, code: Option<i64>, message: &str) -> ExchangeErrorKind {
    if status == StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 418 {
        return ExchangeErrorKind::RateLimited;
    }
    if let Some(kind) = code.and_then(|code| classify_code(code, message)) {
        return kind;
    }
    match status.as_u16() {
        401 => ExchangeErrorKind::Unauthorized,
        // The web application firewall's answer to a client it throttles.
        403 => ExchangeErrorKind::RateLimited,
        // Binance documents a 5xx as an unknown execution status: the
        // request may have succeeded.
        500..=599 => ExchangeErrorKind::OutcomeUnknown,
        _ => ExchangeErrorKind::Other,
    }
}

fn classify_code(code: i64, message: &str) -> Option<ExchangeErrorKind> {
    use ExchangeErrorKind::*;

    let message = message.to_ascii_lowercase();
    let says = |text: &str| message.contains(text);
    Some(match code {
        -1003 | -1015 => RateLimited,
        -1021 => ClockSkew,
        -1002 | -1022 | -2014 | -2015 => Unauthorized,
        -1121 => UnknownSymbol,
        -2013 | -2026 => UnknownOrder,
        -2011 if says("unknown order sent") => UnknownOrder,
        -2010 if says("insufficient balance") => InsufficientBalance,
        -1013 if says("filter failure") => RuleViolation,
        -1111 => RuleViolation,
        -2010 | -2011 | -1013 | -1112 | -1122 | -2016 => Rejected,
        -1199..=-1100 => InvalidRequest,
        -1001 | -1008 | -1016 => Unavailable,
        -1000 | -1006 | -1007 => OutcomeUnknown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ExchangeErrorKind::*;

    fn status(code: u16) -> StatusCode {
        StatusCode::from_u16(code).expect("status")
    }

    // Binance's documented codes, with the messages it sends for them.
    #[test]
    fn every_documented_code_maps_to_its_kind() {
        for (code, message, kind) in [
            (
                -1003,
                "Too much request weight used; current limit is 6000 request weight per 1 MINUTE.",
                RateLimited,
            ),
            (
                -1015,
                "Too many new orders; current limit is 50 orders per 10 SECOND.",
                RateLimited,
            ),
            (
                -1021,
                "Timestamp for this request is outside of the recvWindow.",
                ClockSkew,
            ),
            (
                -1021,
                "Timestamp for this request was 1000ms ahead of the server's time.",
                ClockSkew,
            ),
            (
                -1002,
                "You are not authorized to execute this request.",
                Unauthorized,
            ),
            (
                -1022,
                "Signature for this request is not valid.",
                Unauthorized,
            ),
            (-2014, "API-key format invalid.", Unauthorized),
            (
                -2015,
                "Invalid API-key, IP, or permissions for action.",
                Unauthorized,
            ),
            (-1121, "Invalid symbol.", UnknownSymbol),
            (-2013, "Order does not exist.", UnknownOrder),
            (
                -2026,
                "Order was canceled or expired with no executed qty over 90 days ago and has been archived.",
                UnknownOrder,
            ),
            (-2011, "Unknown order sent.", UnknownOrder),
            (
                -2011,
                "Order was not canceled due to cancel restrictions.",
                Rejected,
            ),
            (
                -2010,
                "Account has insufficient balance for requested action.",
                InsufficientBalance,
            ),
            (-2010, "Duplicate order sent.", Rejected),
            (-2010, "Market is closed.", Rejected),
            (-1013, "Filter failure: NOTIONAL", RuleViolation),
            (-1013, "Filter failure: LOT_SIZE", RuleViolation),
            (-1013, "The request is rejected by the API.", Rejected),
            (
                -1111,
                "Parameter 'quantity' has too much precision.",
                RuleViolation,
            ),
            (-1112, "No orders on book for symbol.", Rejected),
            (-1122, "Invalid symbolStatus.", Rejected),
            (
                -2016,
                "No trading window could be found for the symbol.",
                Rejected,
            ),
            (
                -1100,
                "Illegal characters found in a parameter.",
                InvalidRequest,
            ),
            (
                -1102,
                "Mandatory parameter 'quantity' was not sent, was empty/null, or malformed.",
                InvalidRequest,
            ),
            (
                -1130,
                "Data sent for parameter 'limit' is not valid.",
                InvalidRequest,
            ),
            (
                -1001,
                "Internal error; unable to process your request. Please try again.",
                Unavailable,
            ),
            (
                -1008,
                "Server is currently overloaded with other requests. Please try again in a few minutes.",
                Unavailable,
            ),
            (-1016, "This service is no longer available.", Unavailable),
            (
                -1000,
                "An unknown error occurred while processing the request.",
                OutcomeUnknown,
            ),
            (
                -1006,
                "An unexpected response was received from the message bus. Execution status unknown.",
                OutcomeUnknown,
            ),
            (
                -1007,
                "Timeout waiting for response from backend server. Send status unknown; execution status unknown.",
                OutcomeUnknown,
            ),
        ] {
            assert_eq!(
                classify(status(400), Some(code), message),
                kind,
                "{code} {message:?}"
            );
        }
    }

    #[test]
    fn a_rate_limit_status_wins_over_the_body() {
        assert_eq!(classify(status(429), None, "slow down"), RateLimited);
        assert_eq!(
            classify(
                status(418),
                Some(-1003),
                "Way too much request weight used; IP banned until 1."
            ),
            RateLimited
        );
        assert_eq!(
            classify(status(429), Some(-1121), "Invalid symbol."),
            RateLimited
        );
    }

    // A documented code says more than its status: -1001 arrives as a 5xx
    // but promises the request was not processed.
    #[test]
    fn a_known_code_wins_over_the_status_class() {
        assert_eq!(
            classify(status(500), Some(-1001), "Internal error"),
            Unavailable
        );
        assert_eq!(
            classify(status(503), Some(-1007), "Timeout"),
            OutcomeUnknown
        );
        assert_eq!(classify(status(401), Some(-1021), "Timestamp"), ClockSkew);
    }

    #[test]
    fn without_a_known_code_the_status_class_decides() {
        assert_eq!(classify(status(401), None, ""), Unauthorized);
        assert_eq!(
            classify(status(403), None, "<html>403 Forbidden</html>"),
            RateLimited
        );
        assert_eq!(classify(status(500), None, ""), OutcomeUnknown);
        assert_eq!(
            classify(status(503), None, "<html>503</html>"),
            OutcomeUnknown
        );
        assert_eq!(
            classify(status(503), Some(-9999), "new code"),
            OutcomeUnknown
        );
        assert_eq!(classify(status(400), Some(-9999), "new code"), Other);
        assert_eq!(classify(status(404), None, "not found"), Other);
    }
}
