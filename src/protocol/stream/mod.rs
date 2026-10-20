//! Bars as they close, over a WebSocket.
//!
//! # Session
//!
//! 1. The client opens the WebSocket, at a URL that names no version, and
//!    sends a [`Hello`] with the versions it speaks and its token.
//! 2. The server answers with a [`Welcome`] naming the version the session
//!    runs at. Or it refuses the session with an `error` frame, such as
//!    `unsupported_version` with the versions it speaks, or `unauthorized`,
//!    and closes the connection.
//! 3. The client sends a [`Subscribe`] for each market it wants. The server
//!    refuses one it cannot serve with an `error` frame, and the session goes
//!    on.
//! 4. For each market, [`BarClosed`] frames follow in open-time order, one
//!    per bar. Where the server cannot deliver bars, a [`GapNotice`] says
//!    which and why, so that a client never misses a bar without knowing.
//! 5. Either side may send a [`Ping`], which the other answers with a
//!    [`Pong`] carrying the same nonce. The server pings a session it has
//!    nothing else to send to every [`SessionLimits::ping_interval_ms`], and
//!    closes one it has heard nothing from for
//!    [`SessionLimits::idle_timeout_ms`].
//!
//! # Frames
//!
//! A frame is one WebSocket text message holding a JSON object, and its
//! `type` field names it. A client sends [`ClientFrame`]s and reads
//! [`ServerFrame`]s. A frame whose type a side does not know reads as
//! `Unrecognized`, and is ignored.

mod bars;
mod handshake;

use serde::{Deserialize, Serialize};

use super::ErrorBody;

pub use bars::{Bar, BarClosed, GapNotice, GapReason, Subscribe};
pub use handshake::{AuthToken, Hello, SessionId, SessionLimits, Welcome};

/// A frame the client sends.
///
/// ```
/// use quantforge::protocol::stream::{ClientFrame, Pong};
///
/// let json = r#"{"type":"pong","nonce":42}"#;
/// let frame: ClientFrame = serde_json::from_str(json)?;
/// assert!(matches!(frame, ClientFrame::Pong(Pong { nonce: 42 })));
/// assert_eq!(serde_json::to_string(&frame)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
// No `PartialEq`: a `Hello` holds a token, which is never compared with `==`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientFrame {
    /// Opens the session: the first frame, sent once.
    Hello(Hello),
    /// Asks for one market's closed bars.
    Subscribe(Subscribe),
    /// Checks that the server is still there.
    Ping(Ping),
    /// Answers the server's [`Ping`].
    Pong(Pong),
    /// A frame type this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

/// A frame the server sends.
///
/// ```
/// use quantforge::protocol::ErrorCode;
/// use quantforge::protocol::stream::ServerFrame;
///
/// let json = r#"{"type":"error","code":"not_found","message":"no market binance BTCUSDT 1s"}"#;
/// let frame: ServerFrame = serde_json::from_str(json)?;
/// assert!(matches!(frame, ServerFrame::Error(ref body) if body.code == ErrorCode::NotFound));
/// assert_eq!(serde_json::to_string(&frame)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServerFrame {
    /// Accepts the session.
    Welcome(Welcome),
    /// A closed bar of a subscribed market.
    BarClosed(BarClosed),
    /// Bars of a subscribed market that the server cannot deliver.
    GapNotice(GapNotice),
    /// Checks that the client is still there.
    Ping(Ping),
    /// Answers the client's [`Ping`].
    Pong(Pong),
    /// A refusal: of the session, which then closes, or of one frame, after
    /// which the session goes on. Its fields are an [`ErrorBody`]'s, and are
    /// the same in every version.
    Error(ErrorBody),
    /// A frame type this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

/// Asks the other side to answer with a [`Pong`] carrying the same nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    /// Chosen by the sender, and echoed in the answer.
    pub nonce: u32,
}

/// Answers a [`Ping`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    /// The nonce of the ping it answers.
    pub nonce: u32,
}

#[cfg(test)]
mod tests {
    use super::super::{ErrorCode, ExactDecimal, ProtoVersion, VersionRange};
    use super::*;

    fn decimal(text: &str) -> ExactDecimal {
        text.parse().expect("decimal")
    }

    /// One frame of every type the server sends, with its exact JSON.
    fn server_frames() -> Vec<(ServerFrame, &'static str)> {
        vec![
            (
                ServerFrame::Welcome(Welcome {
                    negotiated_version: ProtoVersion(1),
                    session_id: SessionId::new("s-1").expect("session id"),
                    server_time_ms: 1_792_454_400_123,
                    limits: SessionLimits {
                        max_subscriptions: 16,
                        max_resume_bars: 1_000,
                        ping_interval_ms: 15_000,
                        idle_timeout_ms: 45_000,
                    },
                }),
                r#"{"type":"welcome","negotiated_version":1,"session_id":"s-1","server_time_ms":1792454400123,"limits":{"max_subscriptions":16,"max_resume_bars":1000,"ping_interval_ms":15000,"idle_timeout_ms":45000}}"#,
            ),
            (
                ServerFrame::BarClosed(BarClosed {
                    exchange: "binance".to_string(),
                    symbol: "BTCUSDT".to_string(),
                    interval: "1m".to_string(),
                    bar: Bar {
                        open_time_ms: 1_792_454_400_000,
                        close_time_ms: 1_792_454_459_999,
                        open: decimal("67012.34000000"),
                        high: decimal("67020.00000000"),
                        low: decimal("67001.10000000"),
                        close: decimal("67015.50000000"),
                        volume: decimal("12.04510000"),
                        trades: Some(1_234),
                    },
                }),
                r#"{"type":"bar_closed","exchange":"binance","symbol":"BTCUSDT","interval":"1m","bar":{"open_time_ms":1792454400000,"close_time_ms":1792454459999,"open":"67012.34000000","high":"67020.00000000","low":"67001.10000000","close":"67015.50000000","volume":"12.04510000","trades":1234}}"#,
            ),
            (
                ServerFrame::GapNotice(GapNotice {
                    exchange: "binance".to_string(),
                    symbol: "BTCUSDT".to_string(),
                    interval: "1m".to_string(),
                    first_open_ms: 1_792_454_460_000,
                    last_open_ms: 1_792_454_580_000,
                    reason: GapReason::SlowConsumer,
                }),
                r#"{"type":"gap_notice","exchange":"binance","symbol":"BTCUSDT","interval":"1m","first_open_ms":1792454460000,"last_open_ms":1792454580000,"reason":"slow_consumer"}"#,
            ),
            (
                ServerFrame::Ping(Ping { nonce: 7 }),
                r#"{"type":"ping","nonce":7}"#,
            ),
            (
                ServerFrame::Pong(Pong { nonce: 8 }),
                r#"{"type":"pong","nonce":8}"#,
            ),
            (
                ServerFrame::Error(ErrorBody {
                    code: ErrorCode::UnsupportedVersion,
                    message: "this server speaks protocol versions 1 to 1".to_string(),
                    supported: Some(VersionRange::SUPPORTED),
                }),
                r#"{"type":"error","code":"unsupported_version","message":"this server speaks protocol versions 1 to 1","supported":{"min_supported":1,"proto_version":1}}"#,
            ),
        ]
    }

    /// One frame of every type the client sends, with its exact JSON.
    fn client_frames() -> Vec<(ClientFrame, &'static str)> {
        vec![
            (
                ClientFrame::Hello(Hello {
                    proto_version: ProtoVersion(2),
                    min_supported: ProtoVersion(1),
                    token: AuthToken::new("test-token"),
                    capabilities: vec!["gap_fill".to_string()],
                }),
                r#"{"type":"hello","proto_version":2,"min_supported":1,"token":"test-token","capabilities":["gap_fill"]}"#,
            ),
            (
                ClientFrame::Subscribe(Subscribe {
                    exchange: "binance".to_string(),
                    symbol: "ETHUSDT".to_string(),
                    interval: "5m".to_string(),
                    resume_from_open_ms: Some(1_792_454_100_000),
                }),
                r#"{"type":"subscribe","exchange":"binance","symbol":"ETHUSDT","interval":"5m","resume_from_open_ms":1792454100000}"#,
            ),
            (
                ClientFrame::Ping(Ping { nonce: 1 }),
                r#"{"type":"ping","nonce":1}"#,
            ),
            (
                ClientFrame::Pong(Pong { nonce: 2 }),
                r#"{"type":"pong","nonce":2}"#,
            ),
        ]
    }

    #[test]
    fn every_server_frame_has_its_exact_json() {
        for (frame, json) in server_frames() {
            // Exhaustive on purpose: a new frame type stops this module from
            // compiling until it is added to `server_frames`.
            match &frame {
                ServerFrame::Welcome(_)
                | ServerFrame::BarClosed(_)
                | ServerFrame::GapNotice(_)
                | ServerFrame::Ping(_)
                | ServerFrame::Pong(_)
                | ServerFrame::Error(_)
                | ServerFrame::Unrecognized => {}
            }
            assert_eq!(serde_json::to_string(&frame).expect("serialize"), json);
            assert_eq!(
                serde_json::from_str::<ServerFrame>(json).expect("frame"),
                frame
            );
        }
    }

    // `ClientFrame` has no `PartialEq`, so a frame is compared by its text.
    #[test]
    fn every_client_frame_has_its_exact_json() {
        for (frame, json) in client_frames() {
            match &frame {
                ClientFrame::Hello(_)
                | ClientFrame::Subscribe(_)
                | ClientFrame::Ping(_)
                | ClientFrame::Pong(_)
                | ClientFrame::Unrecognized => {}
            }
            assert_eq!(serde_json::to_string(&frame).expect("serialize"), json);
            let read: ClientFrame = serde_json::from_str(json).expect("frame");
            assert_eq!(serde_json::to_string(&read).expect("serialize"), json);
        }
    }

    #[test]
    fn a_frame_type_this_build_does_not_know_is_unrecognized() {
        let server = r#"{"type":"subscribed","exchange":"binance","symbol":"BTCUSDT"}"#;
        assert_eq!(
            serde_json::from_str::<ServerFrame>(server).expect("frame"),
            ServerFrame::Unrecognized
        );
        let client = r#"{"type":"unsubscribe","exchange":"binance"}"#;
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(client).expect("frame"),
            ClientFrame::Unrecognized
        ));
    }

    #[test]
    fn a_frame_meant_for_the_other_side_is_unrecognized_and_never_misread() {
        for (_, json) in client_frames() {
            let read = serde_json::from_str::<ServerFrame>(json).expect("frame");
            let shared = json.contains(r#""type":"ping""#) || json.contains(r#""type":"pong""#);
            assert_eq!(read == ServerFrame::Unrecognized, !shared, "{json}");
        }
        for (_, json) in server_frames() {
            let read = serde_json::from_str::<ClientFrame>(json).expect("frame");
            let shared = json.contains(r#""type":"ping""#) || json.contains(r#""type":"pong""#);
            assert_eq!(matches!(read, ClientFrame::Unrecognized), !shared, "{json}");
        }
    }

    #[test]
    fn an_unrecognized_frame_is_never_sent() {
        assert!(serde_json::to_string(&ServerFrame::Unrecognized).is_err());
        assert!(serde_json::to_string(&ClientFrame::Unrecognized).is_err());
    }

    #[test]
    fn a_frame_without_a_type_name_is_refused() {
        for json in [
            r#"{"nonce":1}"#,
            r#"{"type":7,"nonce":1}"#,
            r#"{"type":null}"#,
            "[]",
        ] {
            assert!(serde_json::from_str::<ServerFrame>(json).is_err(), "{json}");
            assert!(serde_json::from_str::<ClientFrame>(json).is_err(), "{json}");
        }
    }
}
