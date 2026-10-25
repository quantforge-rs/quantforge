//! What an account's private stream reports (ADR 0002 §11). Streams have no
//! order across each other: one venue numbers its events, the other stamps
//! them with event times. A gap is filled from REST snapshots, and nothing
//! here is evidence that a position is flat.

use super::{
    FuturesFill, FuturesOrder, InstrumentSettings, PositionUpdate, StopOrder, WalletBalance,
};
use crate::model::{InstrumentId, TimestampMs};
use serde::{Deserialize, Serialize};

/// When and in which order an event arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMeta {
    pub received_at_ms: TimestampMs,
    pub venue_time_ms: Option<TimestampMs>,
    /// The venue's sequence number, where it numbers events.
    pub sequence: Option<u64>,
}

/// The stream's connection: losing it and getting it back are both
/// notified, and a restored connection is reconciled before entries resume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionChange {
    Connected,
    Lost,
    Restored,
}

/// The venue's warning that positions are near liquidation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginCall {
    pub instruments: Vec<InstrumentId>,
}

/// One private event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivateEventKind {
    Order(Box<FuturesOrder>),
    Fill(Box<FuturesFill>),
    Position(Box<PositionUpdate>),
    Stop(Box<StopOrder>),
    Wallet(Box<WalletBalance>),
    /// An instrument's settings changed, such as its leverage.
    Config(Box<InstrumentSettings>),
    MarginCall(MarginCall),
    Connection(ConnectionChange),
}

/// An event with when and in which order it arrived.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateEvent {
    pub meta: EventMeta,
    pub kind: PrivateEventKind,
}
