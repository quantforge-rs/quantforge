//! What a delivery is answered with: where its signal stands, and nothing
//! about any order.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::super::ProtocolError;
use super::SIGNAL_VERSION;

/// Where a signal stands when its delivery is answered. These are the only
/// states an answer can give: the one a signal is recorded in and the four
/// it can reach without an order. No receipt says that an order was sent,
/// filled or refused by an exchange, because none has been sent by then.
///
/// ```
/// use quantforge::protocol::signal::ReceiptState;
///
/// let state: ReceiptState = serde_json::from_str(r#""noop""#)?;
/// assert_eq!(state, ReceiptState::Noop);
/// assert_eq!(serde_json::to_string(&state)?, r#""noop""#);
/// // A state of an order is not a receipt's to give.
/// assert!(serde_json::from_str::<ReceiptState>(r#""filled""#).is_err());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    /// Recorded, and not yet judged.
    Received,
    /// Recorded and admitted: an entry that passed admission, or an exit
    /// that will be applied. Nothing has been sized, sent or filled.
    Admitted,
    /// Recorded and refused, for the reason the receipt names.
    Rejected,
    /// A delivery already recorded under the same ID with the same
    /// payload: nothing is done twice.
    Duplicate,
    /// Recorded, with nothing to do: an exit that finds no position of its
    /// own to close.
    Noop,
}

impl ReceiptState {
    /// Every state, in the order a signal's states are listed.
    pub const ALL: [Self; 5] = [
        Self::Received,
        Self::Admitted,
        Self::Rejected,
        Self::Duplicate,
        Self::Noop,
    ];

    /// The state as a receipt writes it, such as `received`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Admitted => "admitted",
            Self::Rejected => "rejected",
            Self::Duplicate => "duplicate",
            Self::Noop => "noop",
        }
    }
}

impl fmt::Display for ReceiptState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The answer to a delivery that was recorded: the body of its `2xx`. It
/// names the signal and where it stands, and never an order. A sender that
/// gets one knows that the delivery will not be lost, and nothing more:
/// whether an order follows is the executor's to decide and to report.
///
/// Like a payload, a receipt is strict: version 1 is exactly these fields.
///
/// ```
/// use quantforge::protocol::signal::{ReceiptState, SignalReceipt};
///
/// let id = "trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z";
/// let receipt = SignalReceipt::received(id);
/// let json = r#"{"version":1,"signal_id":"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z","state":"received"}"#;
/// assert_eq!(serde_json::to_string(&receipt)?, json);
/// assert_eq!(serde_json::from_str::<SignalReceipt>(json)?, receipt);
///
/// // A refusal is recorded too, and says why.
/// let refused = SignalReceipt::rejected(id, "stale")
///     .with_message("the alert fired 75 s ago, and profile main takes entries up to 60 s old");
/// assert_eq!(refused.state(), ReceiptState::Rejected);
/// assert_eq!(
///     serde_json::to_string(&refused)?,
///     r#"{"version":1,"signal_id":"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z","state":"rejected","refusal":"stale","message":"the alert fired 75 s ago, and profile main takes entries up to 60 s old"}"#
/// );
///
/// // Only a rejection carries a refusal, and every rejection does.
/// let unexplained = r#"{"version":1,"signal_id":"s","state":"rejected"}"#;
/// assert!(serde_json::from_str::<SignalReceipt>(unexplained).is_err());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReceiptParts", into = "ReceiptParts")]
pub struct SignalReceipt {
    signal_id: String,
    state: ReceiptState,
    refusal: Option<String>,
    message: Option<String>,
}

impl SignalReceipt {
    fn new(signal_id: impl Into<String>, state: ReceiptState) -> Self {
        Self {
            signal_id: signal_id.into(),
            state,
            refusal: None,
            message: None,
        }
    }

    /// The delivery was recorded, and its signal is not yet judged.
    pub fn received(signal_id: impl Into<String>) -> Self {
        Self::new(signal_id, ReceiptState::Received)
    }

    /// The signal was recorded and admitted.
    pub fn admitted(signal_id: impl Into<String>) -> Self {
        Self::new(signal_id, ReceiptState::Admitted)
    }

    /// The signal was recorded and refused. `refusal` is the refusal's
    /// name, one of QuantForge's, such as `stale` or `unknown_ticker`.
    pub fn rejected(signal_id: impl Into<String>, refusal: impl Into<String>) -> Self {
        Self {
            refusal: Some(refusal.into()),
            ..Self::new(signal_id, ReceiptState::Rejected)
        }
    }

    /// The delivery repeats one already recorded.
    pub fn duplicate(signal_id: impl Into<String>) -> Self {
        Self::new(signal_id, ReceiptState::Duplicate)
    }

    /// The signal was recorded, and there is nothing for it to do.
    pub fn noop(signal_id: impl Into<String>) -> Self {
        Self::new(signal_id, ReceiptState::Noop)
    }

    /// The same receipt with a message for a person: what was refused and
    /// what to change, or why there was nothing to do.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// The ID of the signal the receipt is for, within its profile.
    pub fn signal_id(&self) -> &str {
        &self.signal_id
    }

    /// Where the signal stands.
    pub fn state(&self) -> ReceiptState {
        self.state
    }

    /// Why the signal was refused: the refusal's name. Present exactly
    /// when the state is [`ReceiptState::Rejected`].
    pub fn refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// What a person should know. The wording may change at any time, so a
    /// program reads `state` and `refusal` instead.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// A receipt as it is written, before its rules are checked.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptParts {
    version: u32,
    signal_id: String,
    state: ReceiptState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl From<SignalReceipt> for ReceiptParts {
    fn from(receipt: SignalReceipt) -> Self {
        Self {
            version: SIGNAL_VERSION,
            signal_id: receipt.signal_id,
            state: receipt.state,
            refusal: receipt.refusal,
            message: receipt.message,
        }
    }
}

impl TryFrom<ReceiptParts> for SignalReceipt {
    type Error = ProtocolError;

    fn try_from(parts: ReceiptParts) -> Result<Self, Self::Error> {
        let refuse = |reason: String| Err(ProtocolError::InvalidSignalReceipt { reason });
        if parts.version != SIGNAL_VERSION {
            return refuse(format!(
                "version {} is not supported: this is version {SIGNAL_VERSION} of the format",
                parts.version
            ));
        }
        match (parts.state, &parts.refusal) {
            (ReceiptState::Rejected, None) => {
                return refuse("a rejected signal names its refusal".to_string());
            }
            (state, Some(_)) if state != ReceiptState::Rejected => {
                return refuse(format!(
                    "only a rejected signal carries a refusal, and this one is {state}"
                ));
            }
            _ => {}
        }
        Ok(Self {
            signal_id: parts.signal_id,
            state: parts.state,
            refusal: parts.refusal,
            message: parts.message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "trend:exit:long:SOLUSDT.P:2026-10-20T00:00:00Z";

    #[test]
    fn every_state_is_written_as_its_name() {
        for state in ReceiptState::ALL {
            let json = serde_json::to_string(&state).expect("json");
            assert_eq!(json, format!("\"{}\"", state.as_str()));
            assert_eq!(
                serde_json::from_str::<ReceiptState>(&json).expect("state"),
                state
            );
        }
        for other in [
            "\"expired\"",
            "\"submitting\"",
            "\"outcome_unknown\"",
            "\"not_placed\"",
            "\"partially_filled\"",
            "\"filled\"",
            "\"unfilled\"",
            "\"Received\"",
            "\"\"",
            "1",
        ] {
            assert!(
                serde_json::from_str::<ReceiptState>(other).is_err(),
                "{other}"
            );
        }
    }

    #[test]
    fn each_receipt_is_written_with_its_version_first_and_nothing_it_lacks() {
        let json = |receipt: &SignalReceipt| serde_json::to_string(receipt).expect("json");
        let plain =
            |state: &str| format!("{{\"version\":1,\"signal_id\":\"{ID}\",\"state\":\"{state}\"}}");
        assert_eq!(json(&SignalReceipt::received(ID)), plain("received"));
        assert_eq!(json(&SignalReceipt::admitted(ID)), plain("admitted"));
        assert_eq!(json(&SignalReceipt::duplicate(ID)), plain("duplicate"));
        assert_eq!(json(&SignalReceipt::noop(ID)), plain("noop"));
        let noop = SignalReceipt::noop(ID).with_message("nothing is open");
        assert_eq!(
            json(&noop),
            format!(
                "{{\"version\":1,\"signal_id\":\"{ID}\",\"state\":\"noop\",\"message\":\"nothing is open\"}}"
            )
        );
        let rejected = SignalReceipt::rejected(ID, "conflict");
        assert_eq!(
            json(&rejected),
            format!(
                "{{\"version\":1,\"signal_id\":\"{ID}\",\"state\":\"rejected\",\"refusal\":\"conflict\"}}"
            )
        );
        assert_eq!(rejected.refusal(), Some("conflict"));
        assert_eq!(
            (noop.signal_id(), noop.message()),
            (ID, Some("nothing is open"))
        );
        for receipt in [noop, rejected] {
            let back: SignalReceipt = serde_json::from_str(&json(&receipt)).expect("receipt");
            assert_eq!(back, receipt);
        }
    }

    #[test]
    fn a_receipt_that_breaks_the_format_is_refused() {
        let refused = |json: &str| {
            serde_json::from_str::<SignalReceipt>(json)
                .expect_err("refused")
                .to_string()
        };
        let reason = refused(r#"{"version":1,"signal_id":"s","state":"rejected"}"#);
        assert!(
            reason.contains("a rejected signal names its refusal"),
            "{reason}"
        );
        let reason =
            refused(r#"{"version":1,"signal_id":"s","state":"received","refusal":"stale"}"#);
        assert!(
            reason.contains("only a rejected signal carries a refusal, and this one is received"),
            "{reason}"
        );
        let reason = refused(r#"{"version":2,"signal_id":"s","state":"received"}"#);
        assert!(reason.contains("version 2 is not supported"), "{reason}");
        for broken in [
            r#"{"signal_id":"s","state":"received"}"#,
            r#"{"version":1,"state":"received"}"#,
            r#"{"version":1,"signal_id":"s"}"#,
            r#"{"version":1,"signal_id":"s","state":"filled"}"#,
            r#"{"version":1,"signal_id":"s","state":"received","order_id":"1"}"#,
            r#"{"version":"1","signal_id":"s","state":"received"}"#,
            r#"{"version":1,"signal_id":7,"state":"received"}"#,
        ] {
            assert!(
                serde_json::from_str::<SignalReceipt>(broken).is_err(),
                "{broken}"
            );
        }
    }
}
