use super::*;

/// Inputs `Symbol::new` must accept, paired with the normalized form.
const VALID_SYMBOL_CASES: [(&str, &str); 3] = [
    ("BTCUSDT", "BTCUSDT"),
    ("btcusdt", "BTCUSDT"),
    ("  ethusdt  ", "ETHUSDT"),
];

#[test]
fn symbol_new_trims_uppercases_and_preserves_valid_input() {
    for (input, expected) in VALID_SYMBOL_CASES {
        let symbol = Symbol::new(input).expect("symbol");
        assert_eq!(symbol.as_str(), expected, "for input {input:?}");
    }
}

#[test]
fn empty_symbols_are_rejected_with_exact_message() {
    for input in ["", "   ", "\t\n"] {
        let error = Symbol::new(input).expect_err("symbol error");
        assert!(
            matches!(error, ModelError::InvalidSymbol(_)),
            "for input {input:?}"
        );
        assert_eq!(
            error.to_string(),
            "invalid symbol: empty",
            "for input {input:?}"
        );
    }
}

#[test]
fn symbol_serializes_as_plain_string() {
    let symbol = Symbol::new("BTCUSDT").expect("symbol");
    let json = serde_json::to_string(&symbol).expect("serialize symbol");
    assert_eq!(json, "\"BTCUSDT\"");
}

// Deserialization must apply the same normalization as `Symbol::new`;
// the derived impl used to bypass it entirely on `state_json`/`raw_json`
// reloads from storage.
#[test]
fn symbol_deserialization_normalizes_like_new() {
    for (input, expected) in VALID_SYMBOL_CASES {
        let json = serde_json::to_string(input).expect("encode input");
        let symbol: Symbol = serde_json::from_str(&json).expect("deserialize symbol");
        assert_eq!(symbol.as_str(), expected, "for input {input:?}");
    }
}

#[test]
fn symbol_deserialization_rejects_empty_with_model_error_message() {
    for raw in ["\"\"", "\"  \""] {
        let error = serde_json::from_str::<Symbol>(raw).expect_err("deserialize error");
        assert!(
            error.to_string().contains("invalid symbol: empty"),
            "for input {raw:?}, got {error}"
        );
    }
}

#[test]
fn symbol_round_trips_through_serde_json() {
    let symbol = Symbol::new("BTCUSDT").expect("symbol");
    let json = serde_json::to_string(&symbol).expect("serialize symbol");
    let parsed: Symbol = serde_json::from_str(&json).expect("deserialize symbol");
    assert_eq!(parsed, symbol);
}

/// ADR 0002 §2's spellings for every product line: venue, category,
/// storage name and serde token.
const PRODUCT_LINES: [(ExchangeId, Venue, Category, &str, &str); 6] = [
    (
        ExchangeId::BinanceSpot,
        Venue::Binance,
        Category::Spot,
        "binance_spot",
        "BinanceSpot",
    ),
    (
        ExchangeId::BinanceLinear,
        Venue::Binance,
        Category::Linear,
        "binance_linear",
        "BinanceLinear",
    ),
    (
        ExchangeId::BinanceInverse,
        Venue::Binance,
        Category::Inverse,
        "binance_inverse",
        "BinanceInverse",
    ),
    (
        ExchangeId::BybitSpot,
        Venue::Bybit,
        Category::Spot,
        "bybit_spot",
        "BybitSpot",
    ),
    (
        ExchangeId::BybitLinear,
        Venue::Bybit,
        Category::Linear,
        "bybit_linear",
        "BybitLinear",
    ),
    (
        ExchangeId::BybitInverse,
        Venue::Bybit,
        Category::Inverse,
        "bybit_inverse",
        "BybitInverse",
    ),
];

#[test]
fn every_product_line_keeps_the_adr_spellings_on_both_channels() {
    for (exchange, venue, category, storage, token) in PRODUCT_LINES {
        assert_eq!(exchange.venue(), venue, "{exchange:?}");
        assert_eq!(exchange.category(), category, "{exchange:?}");
        assert_eq!(exchange.as_str(), storage, "{exchange:?}");
        assert_eq!(exchange.to_string(), storage, "{exchange:?}");
        assert_eq!(ExchangeId::from_storage_name(storage), Some(exchange));
        let json = serde_json::to_string(&exchange).expect("serialize exchange");
        assert_eq!(json, format!("\"{token}\""), "{exchange:?}");
        // The two channels never read each other.
        let storage_json = format!("\"{storage}\"");
        assert!(serde_json::from_str::<ExchangeId>(&storage_json).is_err());
        let parsed: ExchangeId = serde_json::from_str(&json).expect("deserialize exchange");
        assert_eq!(parsed, exchange);
    }
}

#[test]
fn from_parts_names_one_line_for_each_venue_and_category() {
    let mut seen = std::collections::HashSet::new();
    for venue in Venue::ALL {
        for category in Category::ALL {
            let exchange = ExchangeId::from_parts(venue, category);
            assert_eq!((exchange.venue(), exchange.category()), (venue, category));
            assert!(seen.insert(exchange), "{exchange:?} named twice");
        }
    }
    assert_eq!(seen.len(), PRODUCT_LINES.len());
}

#[test]
fn venues_and_categories_have_their_wire_names() {
    let venues: Vec<&str> = Venue::ALL.iter().map(|venue| venue.as_str()).collect();
    assert_eq!(venues, ["binance", "bybit"]);
    let categories: Vec<&str> = Category::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(categories, ["spot", "linear", "inverse"]);
    let futures: Vec<bool> = Category::ALL.iter().map(|c| c.is_futures()).collect();
    assert_eq!(futures, [false, true, true]);
}

#[test]
fn all_names_every_product_line_once_venue_by_venue() {
    let lines: Vec<ExchangeId> = ExchangeId::all().collect();
    assert_eq!(lines, PRODUCT_LINES.map(|(exchange, ..)| exchange));
}

#[test]
fn a_category_and_a_product_line_are_read_back_from_their_names() {
    for category in Category::ALL {
        assert_eq!(
            category.as_str().parse::<Category>().expect("category"),
            category
        );
    }
    for (exchange, _, _, storage, _) in PRODUCT_LINES {
        assert_eq!(storage.parse::<ExchangeId>().expect("exchange"), exchange);
    }
}

/// Text that names no category, with the whole message it is refused with.
/// Reading is exact: nothing is trimmed, and case counts.
const INVALID_CATEGORIES: [(&str, &str); 4] = [
    (
        "",
        r#"invalid category "": expected one of spot, linear, inverse"#,
    ),
    (
        "Spot",
        r#"invalid category "Spot": expected one of spot, linear, inverse"#,
    ),
    (
        "spot ",
        r#"invalid category "spot ": expected one of spot, linear, inverse"#,
    ),
    (
        "futures",
        r#"invalid category "futures": expected one of spot, linear, inverse"#,
    ),
];

/// A venue's name alone is not a product line, and neither is a line's
/// serde token: the two channels never read each other.
const INVALID_EXCHANGES: [&str; 6] = [
    "",
    "binance",
    "BinanceSpot",
    "BINANCE_SPOT",
    "binance_spot ",
    "binance-spot",
];

#[test]
fn text_that_names_no_category_or_product_line_is_refused_with_the_names_there_are() {
    for (input, message) in INVALID_CATEGORIES {
        let error = input.parse::<Category>().expect_err("category error");
        assert!(matches!(error, ModelError::InvalidCategory(_)), "{input:?}");
        assert_eq!(error.to_string(), message, "{input:?}");
    }
    for input in INVALID_EXCHANGES {
        let error = input.parse::<ExchangeId>().expect_err("exchange error");
        assert!(matches!(error, ModelError::InvalidExchange(_)), "{input:?}");
        assert_eq!(
            error.to_string(),
            format!(
                "invalid exchange {input:?}: expected one of binance_spot, binance_linear, \
                 binance_inverse, bybit_spot, bybit_linear, bybit_inverse"
            ),
            "{input:?}"
        );
    }
}

#[test]
fn a_market_belongs_to_its_product_lines_category() {
    let symbol = Symbol::new("BTCUSDT").expect("symbol");
    let spot = MarketId::new(ExchangeId::BinanceSpot, symbol.clone(), Interval::M1);
    let linear = MarketId::new(ExchangeId::BybitLinear, symbol, Interval::M1);
    assert_eq!(spot.category(), Category::Spot);
    assert_eq!(linear.category(), Category::Linear);
}

#[test]
fn market_id_json_with_lowercase_symbol_normalizes_on_deserialize() {
    let json = r#"{"exchange":"BinanceSpot","symbol":"btcusdt","interval":"M1"}"#;
    let market: MarketId = serde_json::from_str(json).expect("deserialize market");
    assert_eq!(market.symbol.as_str(), "BTCUSDT");
    assert_eq!(market.interval, Interval::M1);
}
