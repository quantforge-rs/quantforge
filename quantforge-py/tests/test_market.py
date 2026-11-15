"""Market identity: `Symbol`, `Interval`, `Category`, `ExchangeId` and
`MarketId`. Each is built by the engine's own rule and refused in the
engine's own words, and the tables here spell both out, so that they hold
the package to what the engine does and not to what the package says."""

import pytest

import quantforge_py as qf

# What `Symbol` takes, and the symbol it makes of it: the text trimmed and
# its ASCII letters in capitals. An exchange lists symbols in other scripts
# too, which stay as they are.
SYMBOLS = [
    ("BTCUSDT", "BTCUSDT"),
    ("btcusdt", "BTCUSDT"),
    ("  ethusdt  ", "ETHUSDT"),
    ("\t1000pepeusdt\n", "1000PEPEUSDT"),
    ("币安人生usdt", "币安人生USDT"),
    ("btc/usdt", "BTC/USDT"),
]

# Every interval: the constant that names it, its text, and its step in
# milliseconds, written out so that no arithmetic of the engine's is trusted.
INTERVALS = [
    ("S1", "1s", 1_000),
    ("M1", "1m", 60_000),
    ("M3", "3m", 180_000),
    ("M5", "5m", 300_000),
    ("M15", "15m", 900_000),
    ("M30", "30m", 1_800_000),
    ("H1", "1h", 3_600_000),
    ("H2", "2h", 7_200_000),
    ("H4", "4h", 14_400_000),
    ("H6", "6h", 21_600_000),
    ("H8", "8h", 28_800_000),
    ("H12", "12h", 43_200_000),
    ("D1", "1d", 86_400_000),
    ("D3", "3d", 259_200_000),
    ("W1", "1w", 604_800_000),
]

# Text that names no interval, with the message it is refused with. The
# text is trimmed before it is read and before it is quoted; case counts, an
# interval's constant is not its text, and there is no month.
NOT_INTERVALS = [
    ("", "invalid interval: "),
    ("   ", "invalid interval: "),
    ("7m", "invalid interval: 7m"),
    (" 7m ", "invalid interval: 7m"),
    ("8H", "invalid interval: 8H"),
    ("H8", "invalid interval: H8"),
    ("60m", "invalid interval: 60m"),
    ("1min", "invalid interval: 1min"),
    ("1M", "invalid interval: 1M"),
    ("8 h", "invalid interval: 8 h"),
]

# Each product family, and whether it trades futures contracts.
CATEGORIES = [("SPOT", "spot", False), ("LINEAR", "linear", True), ("INVERSE", "inverse", True)]

# Every product line: its constant, the name it is stored under, and its
# family. Venue by venue, and within a venue Spot first.
PRODUCT_LINES = [
    ("BINANCE_SPOT", "binance_spot", "spot"),
    ("BINANCE_LINEAR", "binance_linear", "linear"),
    ("BINANCE_INVERSE", "binance_inverse", "inverse"),
    ("BYBIT_SPOT", "bybit_spot", "spot"),
    ("BYBIT_LINEAR", "bybit_linear", "linear"),
    ("BYBIT_INVERSE", "bybit_inverse", "inverse"),
]

CATEGORY_NAMES = "spot, linear, inverse"
PRODUCT_LINE_NAMES = (
    "binance_spot, binance_linear, binance_inverse, bybit_spot, bybit_linear, bybit_inverse"
)

# Names read exactly: text around a name, a capital, the engine's own name
# for the variant, or a name of another kind, is not the name.
NOT_CATEGORIES = ["", "Spot", "SPOT", " spot", "spot ", "futures", "perpetual"]
NOT_PRODUCT_LINES = [
    "",
    # A venue's name alone is not a product line: nothing takes it for the
    # venue's Spot line.
    "binance",
    "bybit",
    "spot",
    "BinanceSpot",
    "BINANCE_SPOT",
    " binance_spot",
    "binance_spot ",
    "binance-spot",
    "binance_futures",
]


def public(cls) -> list[str]:
    return sorted(name for name in dir(cls) if not name.startswith("_"))


def test_a_symbol_is_its_text_trimmed_and_in_capitals():
    for text, symbol in SYMBOLS:
        assert str(qf.Symbol(text)) == symbol, text


def test_symbols_are_equal_and_hash_alike_when_their_text_is(evaluated):
    for text, symbol in SYMBOLS:
        made = qf.Symbol(text)
        assert made == qf.Symbol(symbol)
        assert hash(made) == hash(qf.Symbol(symbol))
        assert evaluated(repr(made)) == made
    assert repr(qf.Symbol(" btcusdt ")) == "Symbol('BTCUSDT')"
    assert qf.Symbol("BTCUSDT") != qf.Symbol("ETHUSDT")
    assert len({qf.Symbol("btcusdt"), qf.Symbol("BTCUSDT"), qf.Symbol("ETHUSDT")}) == 2


def test_a_symbol_is_never_equal_to_its_text():
    assert qf.Symbol("BTCUSDT") != "BTCUSDT"
    assert "BTCUSDT" != qf.Symbol("BTCUSDT")


def test_a_symbol_that_holds_nothing_is_refused_in_the_engines_words(refused):
    for text in ["", "   ", "\t\n"]:
        refused(ValueError, "invalid symbol: empty", qf.Symbol, text)


def test_a_symbol_is_made_of_text_and_nothing_else(refused):
    for value, named in [(5, "int"), (None, "NoneType"), (b"BTCUSDT", "bytes"), (["BTC"], "list")]:
        refused(TypeError, f"a symbol must be a str, not {named}", qf.Symbol, value)
    with pytest.raises(TypeError):
        qf.Symbol()
    with pytest.raises(TypeError):
        qf.Symbol(value="BTCUSDT")


def test_every_interval_is_a_constant_of_the_class_with_its_text_and_its_step():
    for name, text, step_ms in INTERVALS:
        interval = getattr(qf.Interval, name)
        assert type(interval) is qf.Interval
        assert str(interval) == text
        assert interval.step_ms == step_ms
        assert repr(interval) == f"Interval('{text}')"


def test_the_intervals_are_listed_shortest_first_and_the_class_names_no_other():
    assert qf.Interval.ALL == tuple(getattr(qf.Interval, name) for name, _, _ in INTERVALS)
    assert [interval.step_ms for interval in qf.Interval.ALL] == sorted(
        step_ms for _, _, step_ms in INTERVALS
    )
    assert public(qf.Interval) == sorted([name for name, _, _ in INTERVALS] + ["ALL", "step_ms"])


def test_an_interval_is_read_from_its_text_with_the_space_around_it_trimmed(evaluated):
    for name, text, _ in INTERVALS:
        interval = getattr(qf.Interval, name)
        assert qf.Interval(text) == interval
        assert qf.Interval(f" {text}\n") == interval
        assert hash(qf.Interval(text)) == hash(interval)
        assert evaluated(repr(interval)) == interval
    assert len({qf.Interval(text) for _, text, _ in INTERVALS}) == len(INTERVALS)
    assert qf.Interval("1m") != "1m"


def test_text_that_names_no_interval_is_refused_in_the_engines_words(refused):
    for text, message in NOT_INTERVALS:
        refused(ValueError, message, qf.Interval, text)


def test_an_interval_is_made_of_text_and_nothing_else(refused):
    for value, named in [(60, "int"), (60_000.0, "float"), (None, "NoneType")]:
        refused(TypeError, f"an interval must be a str, not {named}", qf.Interval, value)
    # An interval is not made of another: it is one already.
    refused(TypeError, "an interval must be a str, not Interval", qf.Interval, qf.Interval.M1)


def test_every_product_family_is_a_constant_of_the_class_under_its_name(evaluated):
    for name, text, is_futures in CATEGORIES:
        category = getattr(qf.Category, name)
        assert type(category) is qf.Category
        assert str(category) == text
        assert repr(category) == f"Category('{text}')"
        assert category.is_futures is is_futures
        assert qf.Category(text) == category
        assert hash(qf.Category(text)) == hash(category)
        assert evaluated(repr(category)) == category
    assert qf.Category.ALL == tuple(getattr(qf.Category, name) for name, _, _ in CATEGORIES)
    constants = [name for name, _, _ in CATEGORIES]
    assert public(qf.Category) == sorted(constants + ["ALL", "is_futures"])
    assert qf.Category.SPOT != qf.Category.LINEAR
    assert qf.Category.SPOT != "spot"


def test_text_that_names_no_product_family_is_refused_with_the_three_there_are(refused):
    for text in NOT_CATEGORIES:
        message = f'invalid category "{text}": expected one of {CATEGORY_NAMES}'
        refused(ValueError, message, qf.Category, text)
    refused(TypeError, "a category must be a str, not NoneType", qf.Category, None)


def test_every_product_line_is_a_constant_of_the_class_with_its_family(evaluated):
    for name, text, category in PRODUCT_LINES:
        exchange = getattr(qf.ExchangeId, name)
        assert type(exchange) is qf.ExchangeId
        assert str(exchange) == text
        assert repr(exchange) == f"ExchangeId('{text}')"
        assert type(exchange.category) is qf.Category
        assert exchange.category == qf.Category(category)
        assert qf.ExchangeId(text) == exchange
        assert hash(qf.ExchangeId(text)) == hash(exchange)
        assert evaluated(repr(exchange)) == exchange
    assert qf.ExchangeId.ALL == tuple(getattr(qf.ExchangeId, name) for name, *_ in PRODUCT_LINES)
    constants = [name for name, *_ in PRODUCT_LINES]
    assert public(qf.ExchangeId) == sorted(constants + ["ALL", "category"])
    assert len(set(qf.ExchangeId.ALL)) == len(PRODUCT_LINES)
    assert qf.ExchangeId.BINANCE_SPOT != "binance_spot"


def test_text_that_names_no_product_line_is_refused_with_the_lines_there_are(refused):
    for text in NOT_PRODUCT_LINES:
        message = f'invalid exchange "{text}": expected one of {PRODUCT_LINE_NAMES}'
        refused(ValueError, message, qf.ExchangeId, text)
    refused(TypeError, "an exchange must be a str, not Category", qf.ExchangeId, qf.Category.SPOT)


def test_a_market_is_made_of_its_parts_or_of_their_text():
    typed = qf.MarketId(qf.ExchangeId.BINANCE_SPOT, qf.Symbol("BTCUSDT"), qf.Interval.M1)
    assert typed == qf.MarketId("binance_spot", " btcusdt ", " 1m ")
    assert typed == qf.MarketId(exchange="binance_spot", symbol="BTCUSDT", interval=qf.Interval.M1)
    assert type(typed.exchange) is qf.ExchangeId
    assert type(typed.symbol) is qf.Symbol
    assert type(typed.interval) is qf.Interval
    assert (typed.exchange, typed.symbol, typed.interval) == (
        qf.ExchangeId.BINANCE_SPOT,
        qf.Symbol("BTCUSDT"),
        qf.Interval.M1,
    )


def test_a_markets_family_is_its_product_lines_and_is_never_assumed(refused):
    for _, text, category in PRODUCT_LINES:
        market = qf.MarketId(text, "BTCUSDT", "1h")
        assert market.category == qf.Category(category)
        assert market.exchange == qf.ExchangeId(text)
    # The same symbol on another line is another market.
    assert qf.MarketId("binance_spot", "BTCUSDT", "1h") != qf.MarketId(
        "binance_linear", "BTCUSDT", "1h"
    )
    # A venue's name names no line, so no market: nothing reads it as Spot.
    message = f'invalid exchange "binance": expected one of {PRODUCT_LINE_NAMES}'
    refused(ValueError, message, qf.MarketId, "binance", "BTCUSDT", "1h")


def test_markets_are_equal_and_hash_alike_when_all_three_parts_are(evaluated):
    market = qf.MarketId("bybit_inverse", "BTCUSD", "8h")
    assert repr(market) == "MarketId(ExchangeId('bybit_inverse'), Symbol('BTCUSD'), Interval('8h'))"
    assert evaluated(repr(market)) == market
    assert hash(market) == hash(qf.MarketId("bybit_inverse", "btcusd", " 8h"))
    others = [
        qf.MarketId("bybit_linear", "BTCUSD", "8h"),
        qf.MarketId("bybit_inverse", "ETHUSD", "8h"),
        qf.MarketId("bybit_inverse", "BTCUSD", "1h"),
    ]
    for other in others:
        assert market != other
    assert len({market, evaluated(repr(market)), *others}) == 4
    assert market != ("bybit_inverse", "BTCUSD", "8h")


def test_a_market_is_refused_for_the_part_that_is_wrong_in_that_parts_words(refused):
    refused(ValueError, "invalid symbol: empty", qf.MarketId, "binance_spot", "  ", "1m")
    refused(ValueError, "invalid interval: 7m", qf.MarketId, "binance_spot", "BTCUSDT", "7m")
    refused(
        TypeError,
        "exchange must be an ExchangeId or a str, not Category",
        qf.MarketId,
        qf.Category.SPOT,
        "BTCUSDT",
        "1m",
    )
    refused(
        TypeError,
        "symbol must be a Symbol or a str, not NoneType",
        qf.MarketId,
        "binance_spot",
        None,
        "1m",
    )
    refused(
        TypeError,
        "interval must be an Interval or a str, not int",
        qf.MarketId,
        "binance_spot",
        "BTCUSDT",
        60_000,
    )
    with pytest.raises(TypeError):
        qf.MarketId("binance_spot", "BTCUSDT")


def test_a_value_of_these_types_cannot_be_changed_or_extended():
    market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
    for value, name in [
        (market, "symbol"),
        (market, "category"),
        (qf.Interval.M1, "step_ms"),
        (qf.Category.SPOT, "is_futures"),
        (qf.ExchangeId.BINANCE_SPOT, "category"),
    ]:
        with pytest.raises(AttributeError):
            setattr(value, name, getattr(value, name))
    for value in [market, qf.Symbol("BTCUSDT"), qf.Interval.M1, qf.ExchangeId.BINANCE_SPOT]:
        with pytest.raises(AttributeError):
            value.note = "mine"
