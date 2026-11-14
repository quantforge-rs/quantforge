//! The format's fields: their names, their order, and the rule each
//! value is held to.

use super::super::ExactDecimal;
use super::MAX_TICKER_CHARS;

/// Every field of the format, in the order the canonical payload writes
/// them. A body with any other member is not a signal.
pub const SIGNAL_FIELDS: [&str; 12] = [
    "version",
    "kind",
    "profile",
    "source",
    "ticker",
    "exchange",
    "category",
    "side",
    "bar_time",
    "fire_time",
    "price",
    "stop",
];

/// One field of the format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Version,
    Kind,
    Profile,
    Source,
    Ticker,
    Exchange,
    Category,
    Side,
    BarTime,
    FireTime,
    Price,
    Stop,
}

impl Field {
    /// Every field, in [`SIGNAL_FIELDS`]' order.
    pub(super) const ALL: [Self; 12] = [
        Self::Version,
        Self::Kind,
        Self::Profile,
        Self::Source,
        Self::Ticker,
        Self::Exchange,
        Self::Category,
        Self::Side,
        Self::BarTime,
        Self::FireTime,
        Self::Price,
        Self::Stop,
    ];

    /// The member's name in a body.
    pub(super) const fn name(self) -> &'static str {
        SIGNAL_FIELDS[self as usize]
    }

    pub(super) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.name() == name)
    }

    /// Whether only an entry carries the field.
    pub(super) const fn entry_only(self) -> bool {
        matches!(self, Self::Price | Self::Stop)
    }

    /// Whether a signal of this kind must carry the field: an entry needs
    /// its price, and a body whose kind is not known yet is asked only for
    /// what every signal carries.
    pub(super) const fn required(self, entry: bool) -> bool {
        match self {
            Self::Exchange | Self::Category | Self::Stop => false,
            Self::Price => entry,
            _ => true,
        }
    }

    /// The JSON type of the field's value, for a message.
    pub(super) const fn json_type(self) -> &'static str {
        match self {
            Self::Version => "the number 1",
            _ => "a string",
        }
    }

    /// What the field's value must be, for a message.
    pub(super) const fn rule(self) -> &'static str {
        match self {
            Self::Version => "the number 1",
            Self::Kind => "\"entry\" or \"exit\"",
            Self::Profile => {
                "a profile's name: 1 to 32 of a-z, 0-9, - and _, starting with a letter or a digit"
            }
            Self::Source => {
                "the alert's name: 1 to 32 of a-z, 0-9, - and _, starting with a letter or a digit"
            }
            Self::Ticker => {
                "a ticker as TradingView renders it, such as \"SOLUSDT.P\": 1 to 40 characters, \
                 its letters uppercase, with no exchange before it and no spaces"
            }
            Self::Exchange => {
                "an exchange as TradingView names it, such as \"BYBIT\": 1 to 32 of A-Z, 0-9 and _"
            }
            Self::Category => "\"linear\" or \"inverse\"",
            Self::Side => "\"long\" or \"short\"",
            Self::BarTime | Self::FireTime => {
                "a UTC time, written as TradingView renders it, such as \"2026-10-20T00:15:00Z\""
            }
            Self::Price | Self::Stop => {
                "a price above zero as its digits, such as \"234.23\": no sign, no exponent, at \
                 most 28 decimal places"
            }
        }
    }
}

/// A profile's or an alert's name: 1 to 32 of a-z, 0-9, `-` and `_`,
/// starting with a letter or a digit.
pub(super) fn is_name(text: &str) -> bool {
    let plain = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let mut bytes = text.bytes();
    bytes.next().is_some_and(plain)
        && text.len() <= 32
        && bytes.all(|byte| plain(byte) || byte == b'-' || byte == b'_')
}

/// An exchange as TradingView names one: 1 to 32 of A-Z, 0-9 and `_`.
pub(super) fn is_exchange(text: &str) -> bool {
    let allowed = |byte: u8| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_';
    (1..=32).contains(&text.len()) && text.bytes().all(allowed)
}

/// What a ticker may not hold beyond ASCII, by code point: the C1 controls
/// and every space or invisible format character, so that no two tickers
/// that look alike differ by what cannot be seen.
const TICKER_REFUSED: [(u32, u32); 7] = [
    (0x80, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200F),
    (0x2028, 0x202F),
    (0x205F, 0x206F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];

/// A ticker: 1 to [`MAX_TICKER_CHARS`] characters. Of ASCII it takes A-Z,
/// 0-9, `.`, `_` and `-`; beyond ASCII, any character a venue's symbol may
/// hold but those of [`TICKER_REFUSED`].
pub(super) fn is_ticker(text: &str) -> bool {
    let allowed = |character: char| {
        if character.is_ascii() {
            character.is_ascii_uppercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        } else {
            let point = u32::from(character);
            !TICKER_REFUSED
                .iter()
                .any(|(low, high)| (*low..=*high).contains(&point))
        }
    };
    (1..=MAX_TICKER_CHARS).contains(&text.chars().count()) && text.chars().all(allowed)
}

/// A price: an exact decimal above zero, written without a sign. The
/// decimal's own form refuses the rest: an exponent, a separator, a
/// leading zero, more than 28 decimal places or 96 bits of digits.
pub(super) fn price(text: &str) -> Option<ExactDecimal> {
    if text.starts_with('-') {
        return None;
    }
    let value: ExactDecimal = text.parse().ok()?;
    (!value.get().is_zero()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_its_name_in_the_formats_order() {
        let names: Vec<&str> = Field::ALL.iter().map(|field| field.name()).collect();
        assert_eq!(names, SIGNAL_FIELDS);
        for field in Field::ALL {
            assert_eq!(Field::named(field.name()), Some(field));
        }
        for other in ["", "Version", "qty", "price ", "stop_price"] {
            assert_eq!(Field::named(other), None, "{other:?}");
        }
    }

    #[test]
    fn an_entry_needs_its_price_and_nothing_needs_what_is_optional() {
        let required = |entry| -> Vec<&str> {
            Field::ALL
                .into_iter()
                .filter(|field| field.required(entry))
                .map(Field::name)
                .collect()
        };
        let common = [
            "version",
            "kind",
            "profile",
            "source",
            "ticker",
            "side",
            "bar_time",
            "fire_time",
        ];
        assert_eq!(required(false), common);
        assert_eq!(required(true), [&common[..], &["price"]].concat());
        let entry_only: Vec<&str> = Field::ALL
            .into_iter()
            .filter(|field| field.entry_only())
            .map(Field::name)
            .collect();
        assert_eq!(entry_only, ["price", "stop"]);
    }

    #[test]
    fn a_name_is_lowercase_and_starts_plain() {
        let longest = "p".repeat(32);
        for name in [
            "a",
            "0",
            "main",
            "by-linear",
            "trend_2",
            "take-profit",
            &longest,
        ] {
            assert!(is_name(name), "{name:?}");
        }
        let too_long = "p".repeat(33);
        for name in [
            "",
            "-lead",
            "_lead",
            "Main",
            "a b",
            "a:b",
            "a.b",
            "tr\u{e9}nd",
            "trend\n",
            &too_long,
        ] {
            assert!(!is_name(name), "{name:?}");
        }
    }

    #[test]
    fn an_exchange_is_uppercase() {
        let longest = "X".repeat(32);
        for exchange in ["BYBIT", "BINANCE", "CME_DL", "A", "0X", &longest] {
            assert!(is_exchange(exchange), "{exchange:?}");
        }
        let too_long = "X".repeat(33);
        for exchange in [
            "", "bybit", "Bybit", "BY BIT", "BY-BIT", "BYBIT:", &too_long,
        ] {
            assert!(!is_exchange(exchange), "{exchange:?}");
        }
    }

    #[test]
    fn a_ticker_takes_what_a_venue_lists_and_nothing_unseen() {
        let longest = "T".repeat(MAX_TICKER_CHARS);
        // Forty characters of three bytes each are still forty characters.
        let widest = "\u{5e01}".repeat(MAX_TICKER_CHARS);
        for ticker in [
            "SOLUSDT.P",
            "BTCUSD_PERP",
            "BTC-26DEC26",
            "1000SHIBUSDT",
            "\u{5e01}\u{5b89}\u{4eba}\u{751f}USDT.P",
            "\u{e9}",
            "\u{1f680}USDT",
            &longest,
            &widest,
        ] {
            assert!(is_ticker(ticker), "{ticker:?}");
        }
        let too_long = "T".repeat(MAX_TICKER_CHARS + 1);
        let too_wide = "\u{5e01}".repeat(MAX_TICKER_CHARS + 1);
        for ticker in [
            "",
            "solusdt.p",
            "BYBIT:SOLUSDT.P",
            "SOL USDT",
            "SOL/USDT",
            "SOL\"USDT",
            "SOL\\USDT",
            "BTC1!",
            "{{ticker}}",
            "SOLUSDT\n",
            "SOLUSDT\u{7f}",
            "SOLUSDT\u{80}",
            "SOLUSDT\u{9f}",
            "SOL\u{a0}USDT",
            "SOL\u{1680}USDT",
            "SOL\u{2000}USDT",
            "SOL\u{200b}USDT",
            "SOL\u{200f}USDT",
            "SOL\u{2028}USDT",
            "SOL\u{202e}USDT",
            "SOL\u{205f}USDT",
            "SOL\u{2060}USDT",
            "SOL\u{206f}USDT",
            "SOL\u{3000}USDT",
            "SOL\u{feff}USDT",
            &too_long,
            &too_wide,
        ] {
            assert!(!is_ticker(ticker), "{ticker:?}");
        }
        // The characters on either side of each refused range are taken.
        for edge in [
            '\u{a1}', '\u{167f}', '\u{1681}', '\u{1fff}', '\u{2010}', '\u{2027}', '\u{2030}',
            '\u{205e}', '\u{2070}', '\u{2fff}', '\u{3001}', '\u{fefe}', '\u{ff00}',
        ] {
            assert!(is_ticker(&format!("SOL{edge}")), "{edge:?}");
        }
    }

    #[test]
    fn a_price_is_above_zero_and_exact() {
        let smallest = format!("0.{}1", "0".repeat(27));
        for text in [
            "234.23",
            "234.230",
            "0.1",
            "1",
            "84123.4",
            "79228162514264337593543950335",
            "7.9228162514264337593543950335",
            &smallest,
        ] {
            let value = price(text).expect(text);
            // The digits it was sent with are the digits it keeps.
            assert_eq!(value.to_string(), text);
        }
        let too_precise = format!("1.{}1", "0".repeat(28));
        let too_many_zeros = format!("1.{}", "0".repeat(29));
        for text in [
            "",
            "0",
            "0.000",
            "-234.23",
            "-0",
            "+234.23",
            "2.3423e2",
            "84,123.4",
            "0234.23",
            ".5",
            "234.",
            " 234.23",
            "234.23 ",
            "NaN",
            "na",
            "{{close}}",
            "79228162514264337593543950336",
            &too_precise,
            &too_many_zeros,
        ] {
            assert!(price(text).is_none(), "{text:?}");
        }
    }
}
