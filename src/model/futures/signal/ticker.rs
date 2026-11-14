//! Which instrument a ticker names. There are two spellings and no third:
//! the venue's own symbol, for an asset the profile has settings for, and
//! TradingView's name of a perpetual, for anything the venue lists as one.
//! A ticker that is neither names nothing, whatever it resembles.

use super::super::policy::line_name;
use super::super::{ContractKind, FuturesInstrument, ProfilePolicy};
use super::SignalRefusal;
use crate::model::Refusal;

/// What TradingView appends to a perpetual's ticker: `SOLUSDT.P` is the
/// perpetual of base `SOL` and quote `USDT`, on whichever venue the chart
/// is of.
pub const PERPETUAL_SUFFIX: &str = ".P";

/// How a ticker named its instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TickerSpelling {
    /// The venue's own symbol, such as `BTCUSD_PERP`. It names an asset
    /// only where the profile has settings of its own for it: an alert on
    /// a spot chart renders the same text, and an asset on the profile's
    /// defaults is never traded on the strength of that.
    VenueSymbol,
    /// TradingView's name of a perpetual: its base asset, its quote asset
    /// and [`PERPETUAL_SUFFIX`], such as `BTCUSD.P`.
    TradingView,
}

impl TickerSpelling {
    pub const ALL: [Self; 2] = [Self::VenueSymbol, Self::TradingView];

    /// `venue_symbol` or `tradingview`, as records spell it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VenueSymbol => "venue_symbol",
            Self::TradingView => "tradingview",
        }
    }
}

/// The instrument a ticker names, as its venue lists it, and how the
/// ticker named it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedInstrument<'a> {
    pub instrument: &'a FuturesInstrument,
    pub spelling: TickerSpelling,
}

/// Whether the venue lists the instrument as a perpetual of any kind: what
/// TradingView's suffix names. Whether it may trade is the universe's to
/// say, later.
fn is_perpetual(instrument: &FuturesInstrument) -> bool {
    matches!(
        instrument.spec().contract,
        ContractKind::Perpetual | ContractKind::TradFiPerpetual
    )
}

/// Whether `pair` is the instrument's base asset followed by its quote.
fn is_pair_of(instrument: &FuturesInstrument, pair: &str) -> bool {
    let spec = instrument.spec();
    pair.strip_prefix(spec.base.as_str()) == Some(spec.quote.as_str())
}

impl ProfilePolicy {
    /// The instrument `ticker` names on this profile's product line, among
    /// the instruments its venue lists now.
    ///
    /// - The venue's own symbol names an asset the profile has settings
    ///   for, and nothing else.
    /// - Otherwise a ticker that ends in [`PERPETUAL_SUFFIX`] names the one
    ///   perpetual whose base and quote assets it spells. A list with two
    ///   of them is refused as `ambiguous_selection`, never guessed at.
    /// - Anything else is `unknown_ticker`.
    ///
    /// The same ticker names the same instrument for an entry and for an
    /// exit. Whether an entry may trade it is
    /// [`ProfilePolicy::entry_snapshot`]'s to say.
    pub fn instrument_named<'a>(
        &self,
        ticker: &str,
        listed: &'a [FuturesInstrument],
    ) -> Result<NamedInstrument<'a>, SignalRefusal> {
        let line = self.exchange();
        let profile = self.identity().id();
        let on_line = || listed.iter().filter(|listed| listed.id().exchange == line);
        let by_symbol = on_line().find(|listed| listed.id().symbol.as_str() == ticker);
        if let Some(instrument) = by_symbol {
            if self.asset(&instrument.id().symbol).is_some() {
                return Ok(NamedInstrument {
                    instrument,
                    spelling: TickerSpelling::VenueSymbol,
                });
            }
        }
        let unknown = || {
            let message = if by_symbol.is_some() {
                format!(
                    "{ticker} is the venue's own symbol of a contract profile {profile} has no \
                     settings for; send TradingView's ticker of the perpetual, which ends in \
                     {PERPETUAL_SUFFIX}, or give the asset settings of its own"
                )
            } else {
                format!(
                    "{} lists no contract that {ticker} names; a ticker is TradingView's name \
                     of a perpetual, such as SOLUSDT{PERPETUAL_SUFFIX}, or the venue's own \
                     symbol of an asset profile {profile} has settings for",
                    line_name(line)
                )
            };
            SignalRefusal::new(Refusal::UnknownTicker, message)
        };
        let Some(pair) = ticker.strip_suffix(PERPETUAL_SUFFIX) else {
            return Err(unknown());
        };
        let named: Vec<&FuturesInstrument> = on_line()
            .filter(|listed| is_perpetual(listed) && is_pair_of(listed, pair))
            .collect();
        match named.as_slice() {
            [] => Err(unknown()),
            [only] => Ok(NamedInstrument {
                instrument: only,
                spelling: TickerSpelling::TradingView,
            }),
            several => {
                let symbols: Vec<&str> = several
                    .iter()
                    .map(|listed| listed.id().symbol.as_str())
                    .collect();
                Err(SignalRefusal::new(
                    Refusal::AmbiguousSelection,
                    format!(
                        "{} lists {} perpetuals of {pair}: {}; {ticker} cannot tell them apart",
                        line_name(line),
                        several.len(),
                        symbols.join(", ")
                    ),
                ))
            }
        }
    }
}
