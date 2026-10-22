//! The futures contract of ADR 0002, as types: what a perpetual is and
//! whether universe rule 1 keeps it; an account, its settings and its
//! positions; the orders that enter, close and protect a position; what a
//! venue reports back; and the records a store keeps: an external signal's
//! intent, a position cycle with its protection, and realized PnL.
//!
//! Everything here is venue-neutral. A venue's own fields, such as Bybit's
//! `positionIdx` or Binance's `positionSide`, stay inside its adapter, which
//! maps them onto these types. The ports that serve them are in
//! [`crate::ports::futures`].
//!
//! Two rules run through the types:
//! - **Evidence, not assumption.** An acknowledgement carries no status or
//!   fills, so only an order or a fill read back from the venue moves an
//!   intent to filled; only a read-back of a covering stop makes a position
//!   protected; and only a snapshot without the position closes it.
//! - **Units and assets never mix.** Quantities carry their unit and money
//!   its asset, and a value the venue did not report stays unknown.

mod account;
mod accounting;
mod client_order_id;
mod coverage;
mod events;
mod identity;
mod instrument;
mod intent;
mod intent_state;
mod ledger;
mod order;
mod position;
mod protection_state;
mod stop;

pub use account::{
    AccountKind, AccountSnapshot, Exposure, FeeRates, InstrumentSettings, LeverageBracket,
    Liquidation, LiquidationGap, MarginMode, MarginPool, PoolScope, PositionMode, PositionSnapshot,
    PositionUpdate, WalletBalance,
};
pub use accounting::{AccountingStatus, PnlFigure, PnlSource, RealizedPnl};
pub use client_order_id::{ClientOrderId, OrderLeg};
pub use coverage::{CoverageGap, CoveredStop, RecordedStop, StopOwner};
pub use events::{ConnectionChange, EventMeta, MarginCall, PrivateEvent, PrivateEventKind};
pub use identity::{
    ActivationEpoch, OwnerTag, PositionCycleId, PositionKey, ProfileId, ProfileIdentity, SignalId,
};
pub use instrument::{
    ContractKind, FaceValueSource, FuturesInstrument, InstrumentSpec, InverseContract,
    LeverageFilter, PriceSnapshot, TradingStatus, UNIVERSE_RULE, UniverseRefusal,
};
pub use intent::{
    EntryIntent, ExitIntent, ExternalIntent, INTENT_FORMAT, IntentKind, NotPlacedEvidence,
    PROPAGATION_DELAY_MS, SETTLE_WINDOW_MS,
};
pub use intent_state::IntentState;
pub use ledger::{ClosedPnlRecord, LedgerEntry, LedgerKind, LedgerRecord, TimeWindow};
pub use order::{
    AttachedStop, CancelAttribution, CloseOrder, EntryOrder, FuturesFill, FuturesOrder,
    FuturesOrderStatus, OrderAck, OrderAttempt, OrderLookup, OrderLookupGuarantee, OrderOrigin,
    RecordSource, Rejection,
};
pub use position::{CloseReason, OperatorRemoval, PositionCycle};
pub use protection_state::ProtectionState;
pub use stop::{
    PriceProtection, StopLookup, StopOrder, StopPlacement, StopRef, StopRequest, StopScope,
    StopStatus, TriggerBasis,
};
