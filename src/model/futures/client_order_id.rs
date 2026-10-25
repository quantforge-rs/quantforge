//! QuantForge's own client order IDs (ADR 0002 §2), in layout 1:
//! `qf1-<owner tag>-<epoch>-<operation>-<leg><attempt>`, the numbers in
//! lowercase base 36. An ID is at most 36 characters of `[a-z0-9-]`, which
//! both venues accept. These IDs are not idempotency keys: one venue checks
//! uniqueness only among open orders, and the other's position stops carry
//! no client ID at all.
//!
//! Client IDs read back from a venue stay raw text in the records that
//! carry them, because a venue may accept characters layout 1 never writes.
//! One that does not parse here is not QuantForge's.

use super::{ActivationEpoch, OwnerTag};
use crate::model::{ModelError, OperationId};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// `value` in lowercase base 36, without leading zeros.
pub(super) fn to_base36(mut value: u64) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits
        .iter()
        .rev()
        .map(|&digit| char::from(digit))
        .collect()
}

/// Reads canonical lowercase base 36: no leading zeros, no overflow.
fn from_base36(text: &str) -> Option<u64> {
    if text.is_empty() || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.bytes().try_fold(0u64, |value, byte| {
        let digit = DIGITS.iter().position(|&d| d == byte)?;
        value.checked_mul(36)?.checked_add(digit as u64)
    })
}

/// What an order does within its operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderLeg {
    /// The market entry: the only leg that opens exposure.
    Entry,
    /// A reduce-only close: an exit, or the protective close.
    Close,
    /// The stop covering the whole position.
    Stop,
    /// The reduce-only stop that covers the position while its stop is
    /// replaced.
    Bridge,
}

impl OrderLeg {
    pub const ALL: [Self; 4] = [Self::Entry, Self::Close, Self::Stop, Self::Bridge];

    /// The letter layout 1 writes for the leg.
    pub const fn letter(self) -> char {
        match self {
            Self::Entry => 'e',
            Self::Close => 'c',
            Self::Stop => 's',
            Self::Bridge => 'b',
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Close => "close",
            Self::Stop => "stop",
            Self::Bridge => "bridge",
        }
    }
}

/// A client order ID QuantForge issued: who issued it (owner tag and store
/// epoch), for which operation, which leg and which attempt.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClientOrderId {
    owner: OwnerTag,
    epoch: ActivationEpoch,
    operation: OperationId,
    leg: OrderLeg,
    attempt: u16,
}

impl ClientOrderId {
    /// The prefix that names the layout.
    pub const LAYOUT: &'static str = "qf1";

    /// The longest ID layout 1 writes, which is also the most either venue
    /// accepts.
    pub const MAX_LEN: usize = 36;

    /// The most attempts one operation makes: two base-36 digits. A later
    /// attempt starts a new operation.
    pub const MAX_ATTEMPT: u16 = 1295;

    /// Refuses an attempt outside 1 to [`MAX_ATTEMPT`](Self::MAX_ATTEMPT).
    pub fn new(
        owner: OwnerTag,
        epoch: ActivationEpoch,
        operation: OperationId,
        leg: OrderLeg,
        attempt: u16,
    ) -> Result<Self, ModelError> {
        if !(1..=Self::MAX_ATTEMPT).contains(&attempt) {
            return Err(ModelError::InvalidIdentifier {
                what: "order attempt",
                value: attempt.to_string(),
                rule: "1 to 1295; a later attempt starts a new operation",
            });
        }
        Ok(Self {
            owner,
            epoch,
            operation,
            leg,
            attempt,
        })
    }

    pub fn owner(&self) -> &OwnerTag {
        &self.owner
    }

    pub const fn epoch(&self) -> ActivationEpoch {
        self.epoch
    }

    pub const fn operation(&self) -> OperationId {
        self.operation
    }

    pub const fn leg(&self) -> OrderLeg {
        self.leg
    }

    pub const fn attempt(&self) -> u16 {
        self.attempt
    }

    /// `text` read as layout 1, or `None` when it is someone else's ID.
    pub fn parse_ours(text: &str) -> Option<Self> {
        text.parse().ok()
    }
}

impl fmt::Display for ClientOrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}-{}-{}{}",
            Self::LAYOUT,
            self.owner,
            to_base36(u64::from(self.epoch.0)),
            to_base36(self.operation.get()),
            self.leg.letter(),
            to_base36(u64::from(self.attempt)),
        )
    }
}

/// Reads layout 1 and nothing else: the canonical spelling only, so each
/// ID has one text to compare and look up by.
impl FromStr for ClientOrderId {
    type Err = ModelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ModelError::InvalidIdentifier {
            what: "client order id",
            value: text.to_string(),
            rule: "not a layout-1 ID QuantForge issued",
        };
        let parts: Vec<&str> = text.split('-').collect();
        let &[layout, owner, epoch, operation, leg_attempt] = parts.as_slice() else {
            return Err(invalid());
        };
        if layout != Self::LAYOUT || text.len() > Self::MAX_LEN {
            return Err(invalid());
        }
        let owner = OwnerTag::new(owner).map_err(|_| invalid())?;
        let epoch = from_base36(epoch)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(invalid)?;
        let operation = from_base36(operation)
            .and_then(|value| OperationId::new(value).ok())
            .ok_or_else(invalid)?;
        let mut chars = leg_attempt.chars();
        let letter = chars.next().ok_or_else(invalid)?;
        let leg = OrderLeg::ALL
            .into_iter()
            .find(|leg| leg.letter() == letter)
            .ok_or_else(invalid)?;
        let attempt = from_base36(chars.as_str())
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(invalid)?;
        Self::new(owner, ActivationEpoch(epoch), operation, leg, attempt).map_err(|_| invalid())
    }
}

impl TryFrom<String> for ClientOrderId {
    type Error = ModelError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<ClientOrderId> for String {
    fn from(id: ClientOrderId) -> Self {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(epoch: u32, operation: u64, leg: OrderLeg, attempt: u16) -> ClientOrderId {
        ClientOrderId::new(
            OwnerTag::new("k3x9p2").expect("tag"),
            ActivationEpoch(epoch),
            OperationId::new(operation).expect("operation"),
            leg,
            attempt,
        )
        .expect("client order id")
    }

    /// The characters both venues accept in a client order ID, and how many.
    fn both_venues_accept(text: &str) -> bool {
        (1..=36).contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }

    #[test]
    fn layout_one_writes_and_reads_back_every_part() {
        let entry = id(3, 42, OrderLeg::Entry, 1);
        assert_eq!(entry.to_string(), "qf1-k3x9p2-3-16-e1");
        let parsed: ClientOrderId = entry.to_string().parse().expect("parse");
        assert_eq!(parsed, entry);
        let json = serde_json::to_string(&entry).expect("json");
        assert_eq!(json, "\"qf1-k3x9p2-3-16-e1\"");
        for leg in OrderLeg::ALL {
            let text = id(1, 1, leg, 7).to_string();
            assert_eq!(
                ClientOrderId::parse_ours(&text).map(|id| id.leg()),
                Some(leg)
            );
        }
    }

    #[test]
    fn the_longest_id_is_exactly_thirty_six_characters_both_venues_accept() {
        let longest = id(
            u32::MAX,
            u64::MAX,
            OrderLeg::Bridge,
            ClientOrderId::MAX_ATTEMPT,
        );
        let text = longest.to_string();
        assert_eq!(text.len(), ClientOrderId::MAX_LEN, "{text}");
        assert!(both_venues_accept(&text), "{text}");
        assert_eq!(text.parse::<ClientOrderId>().expect("parse"), longest);
    }

    #[test]
    fn attempts_stop_at_the_two_digit_limit() {
        let tag = OwnerTag::new("k3x9p2").expect("tag");
        let operation = OperationId::new(1).expect("operation");
        for attempt in [0, ClientOrderId::MAX_ATTEMPT + 1] {
            let id = ClientOrderId::new(
                tag.clone(),
                ActivationEpoch(1),
                operation,
                OrderLeg::Close,
                attempt,
            );
            assert!(id.is_err(), "attempt {attempt}");
        }
    }

    #[test]
    fn ids_that_are_not_canonical_layout_one_are_not_ours() {
        for foreign in [
            "autoclose-1729584000000",
            "adl_autoclose",
            "web_abc.def:1/2",
            "qf-tag-run8-ts8-uuid8",
            "qf1-k3x9p2-03-16-e1",
            "qf1-K3X9P2-3-16-e1",
            "qf1-k3x9p2-3-16-x1",
            "qf1-k3x9p2-3-0-e1",
            "qf1-k3x9p2-3-16-e0",
            "qf1-k3x9p2-3-16-e",
            "qf1-k3x9p2-3-16",
            "qf2-k3x9p2-3-16-e1",
            "qf1-k3x9p2-1z141z4-16-e1",
        ] {
            assert!(ClientOrderId::parse_ours(foreign).is_none(), "{foreign}");
        }
    }
}
