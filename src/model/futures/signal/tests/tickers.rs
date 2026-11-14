//! Which instrument a ticker names: the two spellings, and which comes
//! first where a text is both.

use super::*;

#[test]
fn a_ticker_names_an_instrument_by_one_of_two_spellings_and_no_other() {
    let policy = policy();
    let venue = venue();
    let named = |ticker: &str| {
        policy
            .instrument_named(ticker, &venue)
            .map(|named| (named.instrument.id().symbol.to_string(), named.spelling))
            .map_err(|refused| refused.refusal)
    };
    let found = |symbol: &str, spelling| Ok((symbol.to_string(), spelling));
    // The venue's symbol, for the asset the profile has settings for.
    assert_eq!(
        named("SOLUSDT"),
        found("SOLUSDT", TickerSpelling::VenueSymbol)
    );
    assert_eq!(named("BTCUSDT"), Err(Refusal::UnknownTicker));
    // TradingView's name, for any perpetual the venue lists on the line.
    assert_eq!(
        named("SOLUSDT.P"),
        found("SOLUSDT", TickerSpelling::TradingView)
    );
    assert_eq!(
        named("BTCUSDT.P"),
        found("BTCUSDT", TickerSpelling::TradingView)
    );
    // A dated future is not the perpetual, and has no such name.
    assert_eq!(named("BTC-26DEC26"), Err(Refusal::UnknownTicker));
    assert_eq!(named("BTC-26DEC26.P"), Err(Refusal::UnknownTicker));
    // Two perpetuals of one pair are not chosen between.
    assert_eq!(named("XRPUSDT.P"), Err(Refusal::AmbiguousSelection));
    // Another line's contract is no instrument of this profile.
    assert_eq!(named("ETHUSD.P"), Err(Refusal::UnknownTicker));
    assert_eq!(named("ETHUSD"), Err(Refusal::UnknownTicker));
    // Nothing is folded, trimmed or guessed.
    for ticker in [
        "",
        ".P",
        "solusdt.p",
        "SOLUSDT.p",
        "SOLUSDT.PERP",
        "SOLUSDT ",
        "SOL.P",
        "USDT.P",
        "SOLUSDTT.P",
        "SOLUSD.P",
        "USDTSOL.P",
        "SOLUSDT.P.P",
        "BYBIT:SOLUSDT.P",
    ] {
        assert_eq!(named(ticker), Err(Refusal::UnknownTicker), "{ticker:?}");
    }
    // With nothing listed, nothing is named.
    assert_eq!(
        policy
            .instrument_named("SOLUSDT.P", &[])
            .map_err(|refused| refused.refusal),
        Err(Refusal::UnknownTicker)
    );
}

#[test]
fn the_venues_symbol_comes_before_tradingviews_name_where_both_fit() {
    // A venue symbol that itself ends in `.P`: for an asset the profile
    // has settings for, it is that asset, whatever pair it also spells.
    let line = ExchangeId::BybitLinear;
    let odd = listed(line, "SOLUSDT.P", "ODD", "USDT", ContractKind::Perpetual);
    let perpetual = listed(line, "SOLUSDT", "SOL", "USDT", ContractKind::Perpetual);
    let policy = policy();
    let given = policy
        .apply(&PolicyChange::AddAsset(NewAsset {
            instrument: odd.id().clone(),
            enabled: true,
            risk_percent: None,
            stop: StopFallback::Inherit,
            leverage: None,
        }))
        .expect("a change")
        .expect("a revision");
    let venue = [perpetual, odd];
    let named = given.instrument_named("SOLUSDT.P", &venue).expect("named");
    assert_eq!(named.instrument.spec().base, asset("ODD"));
    assert_eq!(named.spelling, TickerSpelling::VenueSymbol);
    // Without settings for it, the same text is TradingView's name.
    let named = policy.instrument_named("SOLUSDT.P", &venue).expect("named");
    assert_eq!(named.instrument.spec().base, asset("SOL"));
    assert_eq!(named.spelling, TickerSpelling::TradingView);
}
