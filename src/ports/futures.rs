//! The futures connector ports of ADR 0002 §1, split by what each may do.
//! Only [`EntryConnector`] opens exposure, and [`ProtectionConnector`] can
//! only reduce it, so a profile that is not armed is given no entry port
//! yet keeps its exits, stop repair and protective close.
//!
//! Every method here is required of every futures adapter, on both venues
//! and in both families. [`FuturesCapabilities`] describes how a venue
//! carries the contract; nothing in it marks an operation unsupported, and
//! a missing operation fails its parity cell. The documentation of these
//! traits is the written contract, which the futures conformance matrix
//! checks against each adapter.
//!
//! Every answer is the venue's, normalized into [`crate::model::futures`]:
//! an acknowledgement is never read as a fill, a value the venue did not
//! report stays unknown, and a venue's own fields, such as its position
//! side or index, never leave its adapter.

use super::{ExchangeError, ExchangeErrorKind};
use crate::model::futures::{
    AccountSnapshot, ClientOrderId, CloseOrder, EntryOrder, FeeRates, FuturesFill,
    FuturesInstrument, FuturesOrder, InstrumentSettings, LedgerRecord, LeverageBracket, OrderAck,
    OrderLookup, OrderLookupGuarantee, PositionSnapshot, PriceSnapshot, PrivateEvent, Rejection,
    StopLookup, StopOrder, StopPlacement, StopRef, StopRequest, TimeWindow,
};
use crate::model::{Environment, ExchangeId, InstrumentId, Leverage};
use async_trait::async_trait;
use std::fmt;
use std::sync::Arc;
use thiserror::Error;

/// How a venue replaces a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StopReplacement {
    /// The stop is changed in place.
    AmendInPlace,
    /// An untriggered stop cannot be changed, so the adapter bridges: it
    /// places a reduce-only stop for the current quantity and verifies it,
    /// cancels the old stop, places and verifies the new one, then cancels
    /// the bridge.
    Bridge,
}

/// Where an account's ID comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccountIdSource {
    /// The venue reports one for the keys in use.
    Venue,
    /// The venue gives none, so a fingerprint of the API key stands in,
    /// which mainnet refuses.
    KeyFingerprint,
}

/// How a venue orders its private events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventOrder {
    /// Each event carries a sequence number.
    Sequence,
    /// Events carry event times, ordered within one kind of event only.
    EventTime,
}

/// How a venue carries the futures contract. It describes and never opts
/// out: no field here can excuse a required operation. [`Default`] is the
/// most conservative description, the one that assumes least of a venue;
/// an adapter starts from it and states what its venue does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FuturesCapabilities {
    pub stop_replacement: StopReplacement,
    /// Whether a stop can be sent with the entry order; otherwise it is
    /// placed after the first fill.
    pub stop_at_entry: bool,
    /// Whether stops carry client IDs; otherwise their ownership comes
    /// from the ledger.
    pub stop_client_ids: bool,
    /// Whether a cancel says who cancelled; otherwise a vanished stop is
    /// repaired.
    pub cancel_attribution: bool,
    pub account_id: AccountIdSource,
    pub event_order: EventOrder,
    /// What a lookup by client ID can prove.
    pub order_lookup: OrderLookupGuarantee,
}

impl Default for FuturesCapabilities {
    fn default() -> Self {
        Self {
            stop_replacement: StopReplacement::Bridge,
            stop_at_entry: false,
            stop_client_ids: false,
            cancel_attribution: false,
            account_id: AccountIdSource::KeyFingerprint,
            event_order: EventOrder::EventTime,
            order_lookup: OrderLookupGuarantee::default(),
        }
    }
}

/// The base every futures connector shares: the product line it trades,
/// the environment it was built for, and how its venue carries the
/// contract.
pub trait FuturesConnector: Send + Sync {
    /// A linear or inverse product line, never Spot.
    fn exchange_id(&self) -> ExchangeId;

    /// The environment the connector was declared for, never read from its
    /// URL. Signed requests go only to that environment's documented hosts.
    fn environment(&self) -> Environment;

    fn capabilities(&self) -> FuturesCapabilities;
}

/// Instrument metadata and prices: public data, read from the connector's
/// own environment, since rules and books differ between environments.
#[async_trait]
pub trait InstrumentConnector: FuturesConnector {
    /// Every instrument the product line lists, whether universe rule 1
    /// keeps it or not.
    async fn instruments(&self) -> Result<Vec<FuturesInstrument>, ExchangeError>;

    /// The instrument's mark, index and book prices, fetched now.
    async fn prices(&self, instrument: &InstrumentId) -> Result<PriceSnapshot, ExchangeError>;
}

/// The account's state: signed, read only.
#[async_trait]
pub trait AccountStateConnector: FuturesConnector {
    /// The account's type, wallets and margin pools.
    async fn account(&self) -> Result<AccountSnapshot, ExchangeError>;

    /// Every open position.
    async fn positions(&self) -> Result<Vec<PositionSnapshot>, ExchangeError>;

    /// One instrument's position, flat or not.
    async fn position(&self, instrument: &InstrumentId) -> Result<PositionSnapshot, ExchangeError>;

    /// The instrument's position mode, margin mode and leverage.
    async fn instrument_settings(
        &self,
        instrument: &InstrumentId,
    ) -> Result<InstrumentSettings, ExchangeError>;

    /// The account's leverage brackets for the instrument.
    async fn leverage_brackets(
        &self,
        instrument: &InstrumentId,
    ) -> Result<Vec<LeverageBracket>, ExchangeError>;

    /// The account's fee rates for the instrument.
    async fn fee_rates(&self, instrument: &InstrumentId) -> Result<FeeRates, ExchangeError>;
}

/// Leverage: a setting that changes margin, never exposure.
#[async_trait]
pub trait LeverageConnector: FuturesConnector {
    /// Sets the instrument's leverage and returns what the venue reads back.
    /// Callers set it only while the position is flat with no orders open.
    async fn set_leverage(
        &self,
        instrument: &InstrumentId,
        leverage: Leverage,
    ) -> Result<Leverage, ExchangeError>;
}

/// The only port that opens exposure.
#[async_trait]
pub trait EntryConnector: FuturesConnector {
    /// Sends a market entry. The answer is an acknowledgement only: its
    /// fills come from [`OrderQueryConnector`] and
    /// [`PrivateEventConnector`]. After an unknown outcome the entry is
    /// looked up, never sent again.
    async fn submit_entry(&self, order: &EntryOrder) -> Result<OrderAck, ExchangeError>;
}

/// The port that can only reduce: a reduce-only close, and the position's
/// stop. Every order it sends is reduce-only, so a duplicate can never add
/// exposure.
#[async_trait]
pub trait ProtectionConnector: FuturesConnector {
    /// Sends a reduce-only market close. Like an entry, its answer is an
    /// acknowledgement only.
    async fn submit_close(&self, order: &CloseOrder) -> Result<OrderAck, ExchangeError>;

    /// Places a stop on the mark price, or reports that the price has
    /// already crossed it, which is final.
    async fn place_stop(&self, stop: &StopRequest) -> Result<StopPlacement, ExchangeError>;

    /// The instrument's stops: its stop inventory, kept apart from
    /// ordinary open orders.
    async fn stops(&self, instrument: &InstrumentId) -> Result<Vec<StopOrder>, ExchangeError>;

    /// One stop, finished ones included.
    async fn stop(
        &self,
        instrument: &InstrumentId,
        stop: &StopRef,
    ) -> Result<StopLookup, ExchangeError>;

    /// Replaces `current` with `replacement` without ever leaving the
    /// position bare, bridging with `bridge` where the venue cannot change
    /// a stop in place. On failure the error lists every stop still in
    /// place, and protection failure handling takes over.
    async fn replace_stop(
        &self,
        current: &StopRef,
        replacement: &StopRequest,
        bridge: &StopRequest,
    ) -> Result<StopRef, StopReplaceError>;

    /// Cancels a stop QuantForge placed; never one the operator placed.
    async fn cancel_stop(&self, stop: &StopRef) -> Result<(), ExchangeError>;
}

/// A stop replacement that stopped part-way, and the stops it left in
/// place.
#[derive(Debug, Error)]
#[error("stop replacement stopped part-way, leaving {} stops in place", .remaining.len())]
pub struct StopReplaceError {
    pub remaining: Vec<StopRef>,
    #[source]
    pub source: ExchangeError,
}

/// Orders, fills and the ledger: signed, read only.
#[async_trait]
pub trait OrderQueryConnector: FuturesConnector {
    /// The order sent with `client_order_id`. "Not found" proves nothing
    /// before ADR 0002 §9's propagation delay, nor outside the venue's
    /// retention.
    async fn order(
        &self,
        instrument: &InstrumentId,
        client_order_id: &ClientOrderId,
    ) -> Result<OrderLookup, ExchangeError>;

    /// The instrument's open ordinary orders, anyone's.
    async fn open_orders(
        &self,
        instrument: &InstrumentId,
    ) -> Result<Vec<FuturesOrder>, ExchangeError>;

    /// The account's fills on the instrument within `window`.
    async fn fills(
        &self,
        instrument: &InstrumentId,
        window: TimeWindow,
    ) -> Result<Vec<FuturesFill>, ExchangeError>;

    /// The account's ledger within `window`: funding, fees, realized PnL,
    /// transfers and any per-close summaries.
    async fn ledger(&self, window: TimeWindow) -> Result<Vec<LedgerRecord>, ExchangeError>;
}

/// The account's private stream.
#[async_trait]
pub trait PrivateEventConnector: FuturesConnector {
    /// The next private event, waiting until one arrives. Losing and
    /// restoring the connection are events too; after a restore the caller
    /// reconciles from snapshots before entries resume.
    async fn next_event(&self) -> Result<PrivateEvent, ExchangeError>;
}

/// Every port of one product line on one account. All are required; which
/// of them a profile may use, such as no entry port while it is not armed,
/// is decided where profiles are routed.
#[derive(Clone)]
pub struct FuturesConnectors {
    pub instruments: Arc<dyn InstrumentConnector>,
    pub account: Arc<dyn AccountStateConnector>,
    pub leverage: Arc<dyn LeverageConnector>,
    pub entry: Arc<dyn EntryConnector>,
    pub protection: Arc<dyn ProtectionConnector>,
    pub orders: Arc<dyn OrderQueryConnector>,
    pub events: Arc<dyn PrivateEventConnector>,
}

impl fmt::Debug for FuturesConnectors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FuturesConnectors")
            .field("exchange", &self.entry.exchange_id())
            .field("environment", &self.entry.environment())
            .finish_non_exhaustive()
    }
}

impl ExchangeError {
    /// The venue's definitive refusal of the order sent as `refused`, when
    /// this error is one: the venue answered and executed nothing. `None`
    /// for a failed transport, an unreadable answer, an unknown outcome or
    /// an unclassified error, any of which may have executed.
    pub fn rejection(&self, refused: &ClientOrderId) -> Option<Rejection> {
        let kind = self.kind();
        match kind {
            ExchangeErrorKind::InvalidRequest
            | ExchangeErrorKind::MissingCredentials
            | ExchangeErrorKind::Unauthorized
            | ExchangeErrorKind::ClockSkew
            | ExchangeErrorKind::RateLimited
            | ExchangeErrorKind::Unavailable
            | ExchangeErrorKind::UnknownSymbol
            | ExchangeErrorKind::UnknownOrder
            | ExchangeErrorKind::InsufficientBalance
            | ExchangeErrorKind::RuleViolation
            | ExchangeErrorKind::Rejected => Some(Rejection::new(
                refused.clone(),
                kind.as_str(),
                self.to_string(),
            )),
            ExchangeErrorKind::Transport
            | ExchangeErrorKind::InvalidResponse
            | ExchangeErrorKind::OutcomeUnknown
            | ExchangeErrorKind::Other => None,
        }
    }
}
