//! The seams every adapter plugs into. An exchange adapter provides
//! [`MarketDataConnector`], [`TradingConnector`] and [`AccountConnector`],
//! all three on the shared [`ExchangeConnector`] base that names the
//! exchange and describes it with [`ExchangeCapabilities`]. A persistence
//! backend provides [`CandleStore`], [`RunJournalStore`] and
//! [`SegmentStore`].
//! [`ExchangeError`] and [`StorageError`] are the two failure channels an
//! engine folds into its own; [`ExchangeErrorKind`] classifies an exchange
//! failure the same way for every exchange.
//!
//! The requests and responses crossing these traits are plain domain types
//! from [`crate::model`]; nothing here names a concrete exchange. Callers
//! obtain connectors from the registry in [`crate::exchange`].

use async_trait::async_trait;
use std::error::Error as StdError;
use std::fmt;
use thiserror::Error;

use crate::model::{
    AccountTrade, AssetBalance, BotRunState, CancelOrderRequest, Candle, ClosedTrade, ExchangeId,
    ExchangeOrder, KlineRequest, MarketId, MarketOrderRequest, ModelError, OrderQueryRequest,
    SegmentHash, SegmentPlan, SegmentRefresh, Symbol, SymbolRules, TimestampMs,
};

#[derive(Clone, Debug, Default)]
pub struct CandleQuery {
    pub start_time_ms: Option<TimestampMs>,
    pub end_time_ms: Option<TimestampMs>,
    pub limit: Option<usize>,
}

#[derive(Error, Debug)]
pub enum ExchangeError {
    #[error("transport error")]
    Transport(#[source] Box<dyn StdError + Send + Sync>),

    /// An error the exchange reported: `kind` is the adapter's
    /// classification of it, `code` the exchange's own error code when it
    /// sent one, and `message` its text. Built with [`ExchangeError::api`],
    /// so a field added later breaks no adapter outside this crate.
    #[error("api error: code={code:?} message={message}")]
    #[non_exhaustive]
    Api {
        kind: ExchangeErrorKind,
        code: Option<i64>,
        message: String,
    },

    #[error("missing credentials")]
    MissingCredentials,

    #[error("invalid response: {message}")]
    InvalidResponse { message: String },

    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
}

impl ExchangeError {
    pub fn transport<E>(err: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self::Transport(Box::new(err))
    }

    /// An error the exchange reported, classified as `kind`.
    pub fn api(kind: ExchangeErrorKind, code: Option<i64>, message: impl Into<String>) -> Self {
        Self::Api {
            kind,
            code,
            message: message.into(),
        }
    }

    /// What kind of failure this is, in the terms every exchange shares.
    pub fn kind(&self) -> ExchangeErrorKind {
        match self {
            Self::Transport(_) => ExchangeErrorKind::Transport,
            Self::Api { kind, .. } => *kind,
            Self::MissingCredentials => ExchangeErrorKind::MissingCredentials,
            Self::InvalidResponse { .. } => ExchangeErrorKind::InvalidResponse,
            Self::InvalidRequest { .. } => ExchangeErrorKind::InvalidRequest,
        }
    }
}

/// What kind of failure an [`ExchangeError`] is, in terms every exchange
/// shares, so a caller reacts the same way whichever exchange failed. Each
/// adapter maps its own error codes onto these kinds, and the conformance
/// suite in [`crate::exchange::conformance`] checks the mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExchangeErrorKind {
    /// No response arrived: connecting, TLS, or the transfer failed. Unless
    /// the connection was never made, an order sent this way may still
    /// have reached the exchange.
    Transport,
    /// The exchange answered with something the connector could not read.
    InvalidResponse,
    /// The request was malformed: refused before it was sent, or rejected
    /// by the exchange for its parameters.
    InvalidRequest,
    /// A signed request was attempted without credentials.
    MissingCredentials,
    /// The exchange rejected the credentials, the signature, or their
    /// permissions.
    Unauthorized,
    /// The request's timestamp fell outside the exchange's window: the
    /// local clock disagrees with the exchange's.
    ClockSkew,
    /// Too many requests or too much request weight; back off before
    /// sending more.
    RateLimited,
    /// The exchange refused the request without processing it, because it
    /// is overloaded or shutting down; sending it again is safe. Reported
    /// only when the exchange guarantees nothing was done, so never for a
    /// server error.
    Unavailable,
    /// The exchange failed while handling the request, which may still have
    /// taken effect: look an order up before sending it again, or it may be
    /// placed twice.
    OutcomeUnknown,
    /// The exchange does not list the symbol.
    UnknownSymbol,
    /// No order matches: it never existed, or a cancel found it no longer
    /// open.
    UnknownOrder,
    /// The account lacks the funds for the order.
    InsufficientBalance,
    /// The order breaks the symbol's trading rules: the quantity step or
    /// limits, the price tick, the minimum notional, or the precision.
    RuleViolation,
    /// The exchange refused the request for another reason, such as a
    /// closed market or a duplicate client order id.
    Rejected,
    /// An exchange error the adapter does not classify.
    Other,
}

impl ExchangeErrorKind {
    /// The kind's name as reports print it, such as `rate_limited`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::InvalidResponse => "invalid_response",
            Self::InvalidRequest => "invalid_request",
            Self::MissingCredentials => "missing_credentials",
            Self::Unauthorized => "unauthorized",
            Self::ClockSkew => "clock_skew",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "unavailable",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::UnknownSymbol => "unknown_symbol",
            Self::UnknownOrder => "unknown_order",
            Self::InsufficientBalance => "insufficient_balance",
            Self::RuleViolation => "rule_violation",
            Self::Rejected => "rejected",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for ExchangeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ModelError> for ExchangeError {
    fn from(err: ModelError) -> Self {
        Self::InvalidResponse {
            message: err.to_string(),
        }
    }
}

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("storage error")]
    Other(#[source] Box<dyn StdError + Send + Sync>),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error(
        "database schema version {found} does not match the supported version {expected}; \
         use a matching quantforge release or re-create the database (candles can be \
         re-synced with `data sync`)"
    )]
    SchemaVersionMismatch { found: String, expected: String },
}

impl StorageError {
    pub fn other<E>(err: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self::Other(Box::new(err))
    }
}

/// What an exchange supports. Each connector reports the capabilities of
/// the exchange behind it, and callers branch on these — never on
/// [`ExchangeId`] — so a second exchange slots in without touching them.
///
/// A capability that is `false` describes a request the connector rejects
/// or cannot serve. [`Default`] is the empty descriptor, supporting
/// nothing; the structs are `#[non_exhaustive]`, so a connector written
/// outside this crate starts from it and switches on what it supports, and
/// a capability added later cannot break it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExchangeCapabilities {
    pub orders: OrderCapabilities,
    pub market_data: MarketDataCapabilities,
    pub streams: StreamCapabilities,
}

/// The orders [`TradingConnector`] can place, and how it identifies them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OrderCapabilities {
    /// Market orders sized in the base asset:
    /// [`MarketOrderRequest::quantity`].
    pub market_by_base_quantity: bool,
    /// Market orders sized in the quote asset:
    /// [`MarketOrderRequest::quote_order_qty`].
    pub market_by_quote_quantity: bool,
    /// Caller-chosen client order ids on new orders, and cancelling or
    /// querying an order by one.
    pub client_order_ids: bool,
}

/// What [`MarketDataConnector`] can serve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MarketDataCapabilities {
    /// The most candles one [`MarketDataConnector::fetch_klines`] call
    /// returns; a larger [`KlineRequest::limit`] is capped to it.
    pub max_klines_per_request: u16,
    /// [`MarketDataConnector::fetch_symbol_rules`]: the exchange's trading
    /// rules for a symbol.
    pub symbol_rules: bool,
    /// [`MarketDataConnector::server_time`]: the exchange's clock.
    pub server_time: bool,
}

/// Pushed updates, as opposed to polling. No connector streams yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamCapabilities {
    /// Candle updates pushed as they form.
    pub klines: bool,
    /// Account and order updates pushed as they happen.
    pub user_data: bool,
}

/// The base every exchange connector shares: which exchange it talks to,
/// what that exchange supports, and whether its endpoint moves real funds.
pub trait ExchangeConnector: Send + Sync {
    fn exchange_id(&self) -> ExchangeId;

    fn capabilities(&self) -> ExchangeCapabilities;

    /// Whether the endpoint is the exchange's production system, where
    /// orders trade real funds — as opposed to a testnet.
    fn is_production(&self) -> bool;
}

/// Public market data: candles, trading rules, and the exchange clock. No
/// credentials needed.
#[async_trait]
pub trait MarketDataConnector: ExchangeConnector {
    async fn fetch_klines(&self, request: &KlineRequest) -> Result<Vec<Candle>, ExchangeError>;
    async fn fetch_symbol_rules(&self, symbol: &Symbol) -> Result<SymbolRules, ExchangeError>;
    async fn server_time(&self) -> Result<TimestampMs, ExchangeError>;
}

/// Placing, cancelling, and looking up orders. Needs credentials.
#[async_trait]
pub trait TradingConnector: ExchangeConnector {
    async fn submit_market_order(
        &self,
        request: &MarketOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError>;
    async fn cancel_order(
        &self,
        request: &CancelOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError>;
    async fn query_order(
        &self,
        request: &OrderQueryRequest,
    ) -> Result<ExchangeOrder, ExchangeError>;
    async fn open_orders(
        &self,
        symbol: Option<&Symbol>,
    ) -> Result<Vec<ExchangeOrder>, ExchangeError>;
}

/// The account: balances and the trade history. Needs credentials.
#[async_trait]
pub trait AccountConnector: ExchangeConnector {
    async fn account_balances(&self) -> Result<Vec<AssetBalance>, ExchangeError>;
    async fn recent_trades(
        &self,
        symbol: &Symbol,
        limit: usize,
    ) -> Result<Vec<AccountTrade>, ExchangeError>;
}

/// The signed half of an exchange, which needs credentials: the live
/// engine and the operator commands both place orders and read the account.
#[derive(Clone, Copy)]
pub struct SignedConnectors<'a> {
    pub trading: &'a dyn TradingConnector,
    pub account: &'a dyn AccountConnector,
}

impl fmt::Debug for SignedConnectors<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedConnectors")
            .field("exchange", &self.trading.exchange_id())
            .finish_non_exhaustive()
    }
}

pub trait CandleStore: Send + Sync {
    fn init(&self) -> Result<(), StorageError>;
    fn upsert_candles(&self, market: &MarketId, candles: &[Candle]) -> Result<usize, StorageError>;
    fn load_candles(
        &self,
        market: &MarketId,
        query: CandleQuery,
    ) -> Result<Vec<Candle>, StorageError>;
    fn load_recent_candles(
        &self,
        market: &MarketId,
        limit: usize,
    ) -> Result<Vec<Candle>, StorageError>;
    fn max_open_time_ms(&self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError>;
}

/// A market's closed candles packed window by window into immutable
/// segments, each named by the hash of its payload, beside the candles they
/// are built from. [`crate::storage::segments`] is the format.
pub trait SegmentStore: Send + Sync {
    /// Packs every closed window whose candles changed since it was last
    /// packed. The window holding the newest candle is the tip, and stays
    /// unpacked.
    fn refresh_segments(&self, market: &MarketId) -> Result<SegmentRefresh, StorageError>;
    /// The segments, and the tip after them, holding `market`'s candles with
    /// open times from `first_open_ms` to `last_open_ms`.
    fn resolve_segments(
        &self,
        market: &MarketId,
        first_open_ms: TimestampMs,
        last_open_ms: TimestampMs,
    ) -> Result<SegmentPlan, StorageError>;
    /// A segment's stored bytes, its payload zstd-compressed, or `None` when
    /// no segment has that name.
    fn load_segment(&self, hash: &SegmentHash) -> Result<Option<Vec<u8>>, StorageError>;
}

pub trait RunJournalStore: Send + Sync {
    fn init(&self) -> Result<(), StorageError>;
    fn save_run_state(&self, state: &BotRunState) -> Result<(), StorageError>;
    fn load_run_state(&self, run_id: &str) -> Result<Option<BotRunState>, StorageError>;
    fn latest_run_for_market(
        &self,
        market: &MarketId,
        strategy_name: &str,
    ) -> Result<Option<BotRunState>, StorageError>;
    fn append_order_event(&self, run_id: &str, order: &ExchangeOrder) -> Result<(), StorageError>;
    fn append_closed_trade(&self, run_id: &str, trade: &ClosedTrade) -> Result<(), StorageError>;
    fn list_order_events(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ExchangeOrder>, StorageError>;
    fn list_closed_trades(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ClosedTrade>, StorageError>;
}
