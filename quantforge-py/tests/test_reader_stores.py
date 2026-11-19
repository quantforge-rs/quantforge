"""What the candle reader reads, on each backend: stores the `quantforge`
command line wrote from an exchange's klines (`stores.py`), a SQLite file
and a PostgreSQL database, read back as batches.

Every candle the command stored comes back digit for digit and oldest
first; a range takes both of its ends, in each of the three ways a time is
written; a market or a range that holds nothing gives an empty batch and
one warning that says what the store holds; product lines of one symbol
stay apart; and a store the reader does not read is refused and left as it
was. Nothing here is particular to a backend but where it says so: the
PostgreSQL tests skip without a server (`conftest.py`).
"""

import csv
import datetime
import gc
import hashlib
import logging
import os
import sqlite3
import threading
import time
import warnings
from decimal import Decimal

import pytest

import quantforge_py as qf
import stores

MARKET = qf.MarketId("binance_spot", "BTCUSDT", "1m")
UTC = datetime.timezone.utc

# What the exchange says of five minutes: prices and a volume as Binance
# writes them, with eight decimal places, and each minute's trade count.
CLOSES = ["81940.01000000", "81955.25000000", "81900.00000000", "81900.10000000", "0.00000001"]
VOLUME = "0.17235000"
COUNTS = [412, 3, 0, 18446744073709551615, 7]


def five_minutes() -> list[list]:
    return [
        stores.kline(index, close, trades=count, volume=VOLUME)
        for index, (close, count) in enumerate(zip(CLOSES, COUNTS))
    ]


def at(index: int) -> int:
    """The open time of the minute `index` after noon."""
    return stores.NOON + index * stores.MINUTE


def minute(index: int) -> datetime.datetime:
    return datetime.datetime(2026, 1, 1, 12, index, tzinfo=UTC)


def written(batch: qf.CandleBatch) -> list[tuple]:
    """Every row of a batch with its decimals as the text they are."""
    columns = [batch.open, batch.high, batch.low, batch.close, batch.volume]
    texts = [[format(value, "f") for value in column] for column in columns]
    return list(zip(batch.open_time_ms, batch.close_time_ms, *texts, batch.trades))


@pytest.fixture
def stored(place):
    """The five minutes, stored by the command line."""
    place.store(five_minutes())
    return place


@pytest.fixture
def reader(stored):
    with qf.CandleReader(**stored.arguments) as reader:
        yield reader


def closes(batch: qf.CandleBatch) -> list[str]:
    return [format(close, "f") for close in batch.close]


def test_what_the_command_line_stored_is_read_digit_for_digit(stored, reader):
    assert (reader.backend, reader.location) == (stored.backend, stored.location)
    batch = reader.load(MARKET)
    assert type(batch) is qf.CandleBatch and batch.market == MARKET
    # Every value as the exchange wrote it: the text of each decimal, with
    # its trailing zeros, the times, and the counts, the largest among them.
    assert written(batch) == [
        (at(index), at(index) + 59_999, close, close, close, close, VOLUME, count)
        for index, (close, count) in enumerate(zip(CLOSES, COUNTS))
    ]
    assert all(type(value) is Decimal for value in batch.close + batch.volume)
    # It is a batch as any other: the one its candles build.
    assert batch == qf.CandleBatch.from_candles(batch.to_candles(), market=MARKET)
    # And reading it again gives it again.
    assert reader.load(MARKET) == batch


def test_the_frozen_day_of_candles_is_read_back_digit_for_digit(place, repository):
    # The goldens' day of candles, as the exchange would have written it:
    # more than it gives in one answer, bars in which nothing traded among
    # them. The command line stores it and the reader reads it back.
    with open(
        repository / "tests" / "fixtures" / "golden" / "candles.csv", newline="", encoding="utf-8"
    ) as file:
        rows = list(csv.DictReader(file))
    assert len(rows) == 1440 and int(rows[0]["open_time_ms"]) == stores.NOON
    prices = ("open", "high", "low", "close", "volume")
    klines = [
        [
            int(row["open_time_ms"]),
            *[row[column] for column in prices],
            int(row["close_time_ms"]),
            "0.00000000",
            int(row["trades"]),
            "0.00000000",
            "0.00000000",
            "0",
        ]
        for row in rows
    ]
    with stores.Exchange({("BTCUSDT", "1m"): klines}) as exchange:
        place.sync(exchange)
        # It took the exchange more than one answer to give the day.
        assert sum(exchange.answers) == 1440 and max(exchange.answers) < 1440
    with qf.CandleReader(**place.arguments) as reader:
        batch = reader.load(MARKET)
        still = reader.load(MARKET, start=at(392), end=at(451))
    # Every value is the text the file has.
    assert written(batch) == [
        (
            int(row["open_time_ms"]),
            int(row["close_time_ms"]),
            *[row[column] for column in prices],
            int(row["trades"]),
        )
        for row in rows
    ]
    # An hour in which nothing traded: a volume of zero with its eight
    # places, and a count of zero that is a count.
    assert len(still) == 60
    assert {format(volume, "f") for volume in still.volume} == {"0.00000000"}
    assert set(still.trades) == {0}


def test_candles_come_oldest_first_however_they_were_stored(place):
    # The later minutes are stored first, as a backfill follows a sync.
    klines = five_minutes()
    place.store(klines[3:])
    place.store(klines[:3])
    with qf.CandleReader(**place.arguments) as reader:
        batch = reader.load(MARKET)
    assert list(batch.open_time_ms) == [at(index) for index in range(5)]
    assert closes(batch) == CLOSES


def test_a_range_takes_both_of_its_ends(reader):
    assert closes(reader.load(MARKET, start=at(1), end=at(3))) == CLOSES[1:4]
    assert closes(reader.load(MARKET, start=at(2), end=at(2))) == CLOSES[2:3]
    # One bound alone, and `None` for a bound, which bounds nothing.
    assert closes(reader.load(MARKET, start=at(3))) == CLOSES[3:]
    assert closes(reader.load(MARKET, end=at(1))) == CLOSES[:2]
    assert closes(reader.load(MARKET, start=None, end=at(1))) == CLOSES[:2]
    assert closes(reader.load(MARKET, start=None, end=None)) == CLOSES
    # A bound a millisecond past a candle leaves that candle out.
    assert closes(reader.load(MARKET, start=at(1) + 1, end=at(3) - 1)) == CLOSES[2:3]
    # And every time the engine holds bounds a range.
    assert closes(reader.load(MARKET, start=-(2**63), end=2**63 - 1)) == CLOSES


def test_a_bound_is_an_int_a_datetime_or_text_and_each_names_the_same_range(reader):
    expected = CLOSES[1:4]
    east = datetime.timezone(datetime.timedelta(hours=5, minutes=30))
    forms = [
        (at(1), at(3)),
        (minute(1), minute(3)),
        # The same instants written in another zone.
        (minute(1).astimezone(east), minute(3).astimezone(east)),
        ("2026-01-01T12:01:00Z", "2026-01-01T12:03:00Z"),
        ("2026-01-01T17:31:00+05:30", "2026-01-01T12:03:00.000Z"),
        ("  2026-01-01T12:01:00Z ", minute(3)),
        (minute(1), at(3)),
    ]
    for start, end in forms:
        assert closes(reader.load(MARKET, start=start, end=end)) == expected, (start, end)


def test_a_datetime_between_two_milliseconds_bounds_the_range_where_it_stands(stored, reader):
    after = minute(1) + datetime.timedelta(microseconds=1)
    before = minute(3) - datetime.timedelta(microseconds=1)
    # A start a microsecond after a candle opened leaves it out, and an end
    # a microsecond before one opened leaves that one out.
    assert closes(reader.load(MARKET, start=after)) == CLOSES[2:]
    assert closes(reader.load(MARKET, end=before)) == CLOSES[:3]
    assert closes(reader.load(MARKET, start=after, end=before)) == CLOSES[2:3]
    # On the millisecond itself, the candle is in.
    whole = minute(1) + datetime.timedelta(microseconds=0)
    assert closes(reader.load(MARKET, start=whole, end=whole)) == CLOSES[1:2]
    # Text between two milliseconds bounds a range the same way, whatever
    # its zone and however long its fraction.
    assert closes(reader.load(MARKET, start="2026-01-01T12:01:00.0001Z")) == CLOSES[2:]
    assert closes(reader.load(MARKET, start="2026-01-01T12:01:00.000000001Z")) == CLOSES[2:]
    assert closes(reader.load(MARKET, start="2026-01-01T17:31:00.0009+05:30")) == CLOSES[2:]
    assert closes(reader.load(MARKET, end="2026-01-01T12:02:59.9999Z")) == CLOSES[:3]
    assert closes(reader.load(MARKET, end="2026-01-01T12:03:00.0009Z")) == CLOSES[:4]
    # A fraction that ends on a millisecond is that millisecond.
    assert closes(reader.load(MARKET, start="2026-01-01T12:01:00.000000Z")) == CLOSES[1:]
    assert closes(reader.load(MARKET, start="2026-01-01T12:00:59.999000Z")) == CLOSES[1:]
    assert closes(reader.load(MARKET, start="2026-01-01T12:00:59.9990001Z")) == CLOSES[1:]
    # A time that ends on a millisecond which is no whole second is that
    # millisecond: a candle that opened on it is in, and one digit more
    # leaves it out.
    stored.sql(stores.candle_row("binance_spot", "BTCUSDT", 9, "5.00", late=1))
    odd = (at(9) + 1,)
    assert reader.load(MARKET, start="2026-01-01T12:09:00.001Z").open_time_ms == odd
    assert reader.load(MARKET, start="2026-01-01T12:09:00.00100Z").open_time_ms == odd
    assert reader.load(MARKET, start="2026-01-01T12:09:00.0010001Z").open_time_ms == ()
    on_it = minute(9) + datetime.timedelta(milliseconds=1)
    assert reader.load(MARKET, start=on_it, end=on_it).open_time_ms == odd
    assert (
        reader.load(MARKET, start=at(9) + 1, end="2026-01-01T12:09:00.001999Z").open_time_ms == odd
    )
    assert reader.load(MARKET, end="2026-01-01T12:09:00.000999Z").open_time_ms[-1] == at(4)
    # The same before the epoch, where a bound rounds the same way.
    early = datetime.datetime(1969, 12, 31, 23, 59, 59, 999_500, tzinfo=UTC)
    assert closes(reader.load(MARKET, start=early, end=minute(0))) == CLOSES[:1]
    assert reader.load(MARKET, end=early) == qf.CandleBatch.from_candles([], market=MARKET)


def test_a_pandas_timestamp_bounds_a_range_with_its_nanoseconds(reader, without_pandas):
    if without_pandas:
        pytest.skip("the run has no pandas")
    import pandas as pd

    # One nanosecond past a candle's open: a `datetime` it is, and more
    # than a microsecond exact.
    after = pd.Timestamp("2026-01-01 12:01:00.000000001", tz="UTC")
    before = pd.Timestamp("2026-01-01 12:02:59.999999999", tz="UTC")
    assert closes(reader.load(MARKET, start=after)) == CLOSES[2:]
    assert closes(reader.load(MARKET, end=before)) == CLOSES[:3]
    on_it = pd.Timestamp("2026-01-01 12:01:00", tz="Asia/Kolkata").tz_convert("UTC")
    assert on_it == pd.Timestamp("2026-01-01 06:31:00", tz="UTC")
    exact = pd.Timestamp(at(1), unit="ms", tz="UTC")
    assert closes(reader.load(MARKET, start=exact, end=exact)) == CLOSES[1:2]
    # A frame's own index bounds the range it came from.
    frame = qf.candles_to_dataframe(reader.load(MARKET))
    again = reader.load(MARKET, start=frame.index[1], end=frame.index[3])
    assert closes(again) == CLOSES[1:4]


REFUSED_BOUNDS = [
    (
        1.5,
        TypeError,
        "{name} must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not float",
    ),
    (
        True,
        TypeError,
        "{name} must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not bool",
    ),
    (
        Decimal(5),
        TypeError,
        "{name} must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not Decimal",
    ),
    (
        b"2026-01-01T12:00:00Z",
        TypeError,
        "{name} must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not bytes",
    ),
    (
        datetime.date(2026, 1, 1),
        TypeError,
        "{name} must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not date",
    ),
    (
        datetime.datetime(2026, 1, 1, 12),
        TypeError,
        "{name} must be a datetime.datetime with a time zone, not one without: a time without a "
        "zone is never read as an instant; give it one, such as datetime.timezone.utc",
    ),
    (
        2**63,
        ValueError,
        "invalid {name} 9223372036854775808: must be between -9223372036854775808 and 9223372036854775807",
    ),
    (
        -(2**63) - 1,
        ValueError,
        "invalid {name} -9223372036854775809: must be between -9223372036854775808 and 9223372036854775807",
    ),
    (
        "",
        ValueError,
        "invalid {name} '': invalid rfc3339 timestamp: empty; write an RFC 3339 time, such as "
        "2026-01-01T00:00:00Z",
    ),
]


@pytest.mark.parametrize("name", ["start", "end"])
@pytest.mark.parametrize(
    "value, kind, message", REFUSED_BOUNDS, ids=[repr(value)[:32] for value, _, _ in REFUSED_BOUNDS]
)
def test_a_bound_of_another_type_or_no_time_is_refused(
    tmp_path, refused, name, value, kind, message
):
    place = stores.Place.sqlite(tmp_path)
    place.store(five_minutes())
    with qf.CandleReader(**place.arguments) as reader:
        refused(kind, message.format(name=name), reader.load, MARKET, **{name: value})


@pytest.mark.parametrize(
    "text",
    ["2026-01-01", "2026-01-01T12:00:00", "noon", "1767268800000", "2026-13-01T00:00:00Z"],
)
def test_text_that_is_no_rfc_3339_time_is_refused_as_the_command_line_refuses_it(tmp_path, text):
    place = stores.Place.sqlite(tmp_path)
    place.store(five_minutes())
    with qf.CandleReader(**place.arguments) as reader:
        for name in ("start", "end"):
            with pytest.raises(ValueError) as caught:
                reader.load(MARKET, **{name: text})
            said = str(caught.value)
            assert said.startswith(f"invalid {name} '{text}': time parse error: "), said
            assert said.endswith("; write an RFC 3339 time, such as 2026-01-01T00:00:00Z"), said


def nothing(market: qf.MarketId) -> qf.CandleBatch:
    return qf.CandleBatch.from_candles([], market=market)


def test_a_market_or_a_range_that_holds_nothing_gives_an_empty_batch_and_one_warning(place, caplog):
    # Minutes 0, 1 and 4: a hole where 2 and 3 would be.
    klines = five_minutes()
    place.store(klines[:2])
    place.store(klines[4:])
    held = "it holds them from 2026-01-01T12:00:00Z to 2026-01-01T12:04:00Z"
    spot = "binance_spot BTCUSDT 1m"
    cases = [
        # A market the store holds nothing of: another symbol, another
        # interval, another venue, another family, whatever the range.
        (
            qf.MarketId("binance_spot", "ETHUSDT", "1m"),
            {},
            "holds no candles of binance_spot ETHUSDT 1m",
        ),
        (
            qf.MarketId("binance_spot", "BTCUSDT", "1h"),
            {},
            "holds no candles of binance_spot BTCUSDT 1h",
        ),
        (
            qf.MarketId("bybit_spot", "BTCUSDT", "1m"),
            {},
            "holds no candles of bybit_spot BTCUSDT 1m",
        ),
        (
            qf.MarketId("binance_linear", "BTCUSDT", "1m"),
            dict(start=at(0), end=at(4)),
            "holds no candles of binance_linear BTCUSDT 1m",
        ),
        # A range after the last candle, before the first, and in the hole.
        (
            MARKET,
            dict(start=at(5), end=at(9)),
            f"holds no candles of {spot} from 2026-01-01T12:05:00Z to 2026-01-01T12:09:00Z; {held}",
        ),
        (
            MARKET,
            dict(start="2025-12-31T00:00:00Z", end=minute(0) - datetime.timedelta(seconds=1)),
            f"holds no candles of {spot} from 2025-12-31T00:00:00Z to 2026-01-01T11:59:59Z; {held}",
        ),
        (
            MARKET,
            dict(start=at(2), end=at(3)),
            f"holds no candles of {spot} from 2026-01-01T12:02:00Z to 2026-01-01T12:03:00Z; {held}",
        ),
        # A range of one instant, which is a range, and holds no candle.
        (
            MARKET,
            dict(start=at(3), end=at(3)),
            f"holds no candles of {spot} from 2026-01-01T12:03:00Z to 2026-01-01T12:03:00Z; {held}",
        ),
        # One bound alone.
        (
            MARKET,
            dict(start=at(4) + 1),
            f"holds no candles of {spot} from 2026-01-01T12:04:00.001Z on; {held}",
        ),
        (
            MARKET,
            dict(end=at(0) - 1),
            f"holds no candles of {spot} up to 2026-01-01T11:59:59.999Z; {held}",
        ),
        # A range that ends before it starts holds no time.
        (
            MARKET,
            dict(start=at(4), end=at(0)),
            f"holds no candles of {spot} from 2026-01-01T12:04:00Z to 2026-01-01T12:00:00Z: "
            "the range ends before it starts",
        ),
    ]
    with qf.CandleReader(**place.arguments) as reader:
        for market, bounds, said in cases:
            caplog.clear()
            with caplog.at_level(logging.DEBUG), warnings.catch_warnings():
                # No exception, and no warning of Python's that a run could
                # turn into one.
                warnings.simplefilter("error")
                batch = reader.load(market, **bounds)
            assert batch == nothing(market) and len(batch) == 0 and batch.market == market
            (record,) = caplog.records
            assert (record.name, record.levelno) == ("quantforge_py", logging.WARNING)
            assert record.getMessage() == f"{place.location} {said}", bounds
        # A range that holds a candle says nothing.
        caplog.clear()
        with caplog.at_level(logging.DEBUG):
            assert len(reader.load(MARKET)) == 3
            assert len(reader.load(MARKET, start=at(1), end=at(1))) == 1
        assert caplog.records == []


def test_the_warning_is_silenced_as_any_logger_is(stored, reader, caplog):
    logger = logging.getLogger("quantforge_py")
    logger.setLevel(logging.ERROR)
    try:
        with caplog.at_level(logging.DEBUG, logger="root"):
            assert len(reader.load(MARKET, start=at(9))) == 0
        assert caplog.records == []
    finally:
        logger.setLevel(logging.NOTSET)


def test_product_lines_and_venues_of_one_symbol_stay_apart(stored, reader):
    # No command stores a line but Binance's Spot yet, so the others are
    # written here: one candle each, at the same minute, of the same symbol
    # and interval, told apart by its close.
    lines = [str(line) for line in qf.ExchangeId.ALL]
    assert len(lines) == 6 and lines[0] == "binance_spot"
    for index, line in enumerate(lines[1:], start=1):
        stored.sql(stores.candle_row(line, "BTCUSDT", 0, f"{index}.5"))
    for index, line in enumerate(lines[1:], start=1):
        batch = reader.load(qf.MarketId(line, "BTCUSDT", "1m"))
        assert batch.market.exchange == qf.ExchangeId(line)
        assert written(batch) == [
            (
                at(0),
                at(0) + 59_999,
                f"{index}.5",
                f"{index}.5",
                f"{index}.5",
                f"{index}.5",
                "1",
                None,
            )
        ], line
    # And Spot holds what it held: nothing of another line reached it.
    assert closes(reader.load(MARKET)) == CLOSES


def test_a_count_the_store_does_not_hold_is_unknown_and_never_zero(stored, reader):
    stored.sql(stores.candle_row("binance_spot", "BTCUSDT", 9, "5.00"))
    batch = reader.load(MARKET, start=at(4))
    assert batch.trades == (7, None)
    assert batch.to_candles()[1].trades is None


def test_stored_text_that_no_decimal_holds_exactly_is_refused_and_never_rounded(
    stored, reader, refused
):
    # One digit more than the engine's decimal has places for.
    long = "1." + "0" * 28 + "1"
    stored.sql(f"UPDATE candles SET close = '{long}' WHERE open_time_ms = {at(2)}")
    with pytest.raises(ValueError) as caught:
        reader.load(MARKET)
    assert str(caught.value).startswith(f'stored decimal "{long}" cannot be read exactly: ')
    # The candles before it are still read.
    assert closes(reader.load(MARKET, end=at(1))) == CLOSES[:2]


def test_what_a_store_fails_with_is_a_runtime_error_that_says_what_the_store_said(
    stored, reader, refused
):
    # A store whose candles are gone from under the reader: nothing a retry
    # mends, and no refusal of the reader's own.
    stored.sql("ALTER TABLE candles RENAME TO candles_gone")
    said = {
        "sqlite": (
            "storage error: no such table: candles: Error code 1: SQL error or missing database"
        ),
        "postgres": 'storage error: db error: ERROR: relation "candles" does not exist',
    }[stored.backend]
    refused(RuntimeError, said, reader.load, MARKET)
    # The reader is none the worse for it.
    stored.sql("ALTER TABLE candles_gone RENAME TO candles")
    assert closes(reader.load(MARKET)) == CLOSES


def test_the_reader_sees_what_the_command_line_stored_since_it_opened(place):
    klines = five_minutes()
    place.store(klines[:2])
    with qf.CandleReader(**place.arguments) as reader:
        assert closes(reader.load(MARKET)) == CLOSES[:2]
        place.store(klines[2:])
        assert closes(reader.load(MARKET)) == CLOSES


class EnterThenExit(qf.Strategy):
    """Goes long on the first bar and flat on the third."""

    def __init__(self):
        self.bars = 0

    def on_bar(self, ctx, bar):
        self.bars += 1
        if self.bars == 1:
            return qf.TargetPosition.LONG_ALL_IN
        if self.bars == 3:
            return qf.TargetPosition.FLAT
        return None


def test_what_is_read_runs_and_converts_as_any_batch_does(reader, without_numpy, without_pandas):
    batch = reader.load(MARKET, end=at(3))
    config = qf.BacktestConfig(initial_cash=10_000, fee_bps=0, close_out_at_end=True)
    # On the market the batch names: none is given.
    result = qf.run_backtest(batch, EnterThenExit(), config)
    built = qf.CandleBatch.from_candles(batch.to_candles(), market=MARKET)
    assert result == qf.run_backtest(built, EnterThenExit(), config)
    assert result.trade_count == 1 and result.trades[0].entry_time_ms == at(1)
    if without_numpy:
        return
    arrays = batch.to_numpy()
    assert arrays["open_time_ms"].tolist() == [at(index) for index in range(4)]
    assert arrays["close"].tolist() == [float(close) for close in CLOSES[:4]]
    assert qf.CandleBatch.from_numpy(**arrays, market=batch.market) == batch
    if without_pandas:
        return
    frame = qf.candles_to_dataframe(batch)
    assert frame.index.astype("int64").tolist() == [at(index) for index in range(4)]
    assert frame["close"].tolist() == [float(close) for close in CLOSES[:4]]
    assert qf.dataframe_to_candles(frame, market=batch.market) == batch


def test_both_backends_give_the_same_batch(tmp_path, postgres_server):
    (tmp_path / "sqlite").mkdir()
    (tmp_path / "postgres").mkdir()
    sqlite = stores.Place.sqlite(tmp_path / "sqlite")
    with stores.Database(postgres_server) as database:
        postgres = stores.Place.postgres(database, tmp_path / "postgres")
        batches = []
        for place in (sqlite, postgres):
            place.store(five_minutes())
            # A candle without a count, which no command stores.
            place.sql(stores.candle_row("binance_spot", "BTCUSDT", 9, "5.00"))
            with qf.CandleReader(**place.arguments) as reader:
                assert reader.backend == place.backend
                batches.append(reader.load(MARKET))
    one, other = batches
    assert one == other and written(one) == written(other) and len(one) == 6


def fingerprint(place: stores.Place) -> str:
    """What a store holds that a reader could have changed: its schema
    version, its migrations, and every candle."""
    rows = place.sql(
        "SELECT CAST(open_time_ms AS TEXT), open, high, low, close, volume, "
        "CAST(trades AS TEXT), exchange, symbol, interval FROM candles "
        "ORDER BY exchange, symbol, interval, open_time_ms"
    )
    rows += place.sql("SELECT key, value FROM meta ORDER BY key")
    rows += place.sql("SELECT CAST(count(*) AS TEXT) FROM schema_migrations")
    return hashlib.sha256(repr(rows).encode()).hexdigest()


def test_reading_changes_nothing_in_the_store(stored):
    before = fingerprint(stored)
    tables = stored.tables()
    if stored.backend == "sqlite":
        file_before = stored.path.read_bytes()
    with qf.CandleReader(**stored.arguments) as reader:
        assert len(reader.load(MARKET)) == 5
        assert len(reader.load(qf.MarketId("bybit_spot", "BTCUSDT", "1m"))) == 0
        assert len(reader.load(MARKET, start=at(9))) == 0
    assert fingerprint(stored) == before and stored.tables() == tables
    if stored.backend == "sqlite":
        # The file itself, byte for byte. Beside it SQLite may leave what it
        # has any reader of such a database leave: a log with nothing in it,
        # and the index of that log.
        assert stored.path.read_bytes() == file_before
        beside = {path.name: path.stat().st_size for path in stored.directory.iterdir()}
        assert beside.pop("market.sqlite") == len(file_before)
        assert set(beside) <= {"market.sqlite-wal", "market.sqlite-shm"}
        assert beside.get("market.sqlite-wal", 0) == 0


def test_a_reader_changes_nothing_of_a_store_another_writer_left_unchecked(tmp_path):
    # A writer killed after it committed a candle leaves it in the log, and
    # nothing moved it into the database: a reader reads it, and a reader
    # that could write would move it in as it closed.
    place = stores.Place.sqlite(tmp_path)
    place.store(five_minutes())
    late = stores.candle_row("binance_spot", "BTCUSDT", 9, "5.00")
    stores.die_writing(place.path, "wal", [late])
    wal = place.directory / "market.sqlite-wal"
    before, logged = place.path.read_bytes(), wal.read_bytes()
    assert len(logged) > 0
    with qf.CandleReader(**place.arguments) as reader:
        batch = reader.load(MARKET)
    assert len(batch) == 6 and batch.open_time_ms[-1] == at(9)
    assert place.path.read_bytes() == before
    assert wal.read_bytes() == logged


def test_a_reader_changes_nothing_of_a_store_in_rollback_mode(tmp_path):
    # A store an operator switched to a rollback journal: a reader that
    # could write would switch it back to a log as it opened.
    place = stores.Place.sqlite(tmp_path)
    place.store(five_minutes())
    place.sql("PRAGMA journal_mode=DELETE")
    before = place.path.read_bytes()
    with qf.CandleReader(**place.arguments) as reader:
        assert closes(reader.load(MARKET)) == CLOSES
    assert place.path.read_bytes() == before
    assert sorted(path.name for path in place.directory.iterdir()) == ["market.sqlite"]
    assert place.value("PRAGMA journal_mode") == "delete"


def test_a_lock_another_process_holds_is_waited_for_and_one_held_too_long_is_a_connection_error(
    tmp_path,
):
    place = stores.Place.sqlite(tmp_path)
    place.store(five_minutes())
    with qf.CandleReader(**place.arguments) as reader:
        # Let go within the wait: the load waits for it, and reads.
        with stores.Holder(place.path) as holder:
            letting_go = threading.Timer(0.5, holder.release)
            letting_go.start()
            started = time.monotonic()
            assert closes(reader.load(MARKET)) == CLOSES
            assert time.monotonic() - started >= 0.4
            letting_go.join()
        # Held past the wait: a load, and opening a reader, each give up on
        # it after five seconds, as a ConnectionError: nothing was read, and
        # the same call reads once the lock is let go.
        outcomes = {}

        def attempt(name, call):
            started = time.monotonic()
            try:
                call()
                outcomes[name] = ("read", time.monotonic() - started)
            except BaseException as failure:  # noqa: BLE001: what came of it
                outcomes[name] = (failure, time.monotonic() - started)

        with stores.Holder(place.path):
            threads = [
                threading.Thread(target=attempt, args=("load", lambda: reader.load(MARKET))),
                threading.Thread(
                    target=attempt, args=("open", lambda: qf.CandleReader(**place.arguments))
                ),
            ]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(30)
        busy = "storage busy: another writer held the database past the wait"
        expected = {
            "load": busy,
            "open": f"cannot open the sqlite store at {place.location}: {busy}",
        }
        for name, said in expected.items():
            failure, waited = outcomes[name]
            assert type(failure) is ConnectionError and str(failure) == said, name
            assert 4.5 <= waited < 15, (name, waited)
        assert closes(reader.load(MARKET)) == CLOSES


def test_a_journal_a_writer_left_unfinished_is_refused_with_the_command_that_rolls_it_back(
    tmp_path, refused
):
    place = stores.Place.sqlite(tmp_path)
    place.store(five_minutes())
    rows = [
        stores.candle_row("binance_spot", "BTCUSDT", 10 + index, "5.00") for index in range(300)
    ]
    stores.die_writing(place.path, "journal", rows)
    assert (place.directory / "market.sqlite-journal").stat().st_size > 0
    refused(
        ValueError,
        f"{place.location} holds the journal of a write that never finished, which only a writer "
        "rolls back; let a command of the quantforge command line open it first, such as "
        f"`quantforge --db {place.location} store status`",
        qf.CandleReader,
        **place.arguments,
    )
    # The command it names rolls the write back, and the reader reads what
    # was there before it.
    stores.quantforge("--db", place.location, "store", "status", cwd=tmp_path)
    with qf.CandleReader(**place.arguments) as reader:
        assert closes(reader.load(MARKET)) == CLOSES


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="no named pipes on this platform")
def test_what_is_no_file_is_refused_before_it_is_opened(tmp_path, refused):
    # A pipe, which opening would wait on for good, and a device.
    pipe = tmp_path / "market.sqlite"
    os.mkfifo(pipe)
    refused(
        ValueError,
        f"{pipe} is no file, and a SQLite database is a file",
        qf.CandleReader,
        db=pipe,
    )
    if os.path.exists("/dev/null"):
        refused(
            ValueError,
            "/dev/null is no file, and a SQLite database is a file",
            qf.CandleReader,
            db="/dev/null",
        )
    # A file in a directory that may not be entered may well be there.
    if hasattr(os, "geteuid") and os.geteuid() != 0:
        place = stores.Place.sqlite(tmp_path / "closed")
        (tmp_path / "closed").mkdir()
        place.store(five_minutes())
        (tmp_path / "closed").chmod(0)
        try:
            with pytest.raises(ValueError) as caught:
                qf.CandleReader(**place.arguments)
        finally:
            (tmp_path / "closed").chmod(0o755)
        assert str(caught.value).startswith(f"cannot reach {place.location}: Permission denied")


def test_a_reader_alone_reads_whole_right_batches_while_the_command_line_writes(place):
    # The command line stores minute after minute in another process, and
    # the reader loads all the while: every batch is the minutes stored so
    # far, every value right, and none fewer than the batch before.
    place.store([stores.kline(0, "81000.00000000")])
    stop = threading.Event()
    stored = []

    def storing():
        minute = 1
        while not stop.is_set() and minute < 400:
            place.store(
                [
                    stores.kline(index, f"{81000 + index}.00000000")
                    for index in range(minute, minute + 20)
                ]
            )
            minute += 20
            stored.append(minute)

    writer = threading.Thread(target=storing, daemon=True)
    writer.start()
    seen, loads = 0, 0
    with qf.CandleReader(**place.arguments) as reader:
        deadline = time.monotonic() + 1.5
        while time.monotonic() < deadline or loads < 20:
            batch = reader.load(MARKET)
            count = len(batch)
            assert list(batch.open_time_ms) == [at(index) for index in range(count)]
            assert closes(batch) == [f"{81000 + index}.00000000" for index in range(count)]
            assert count >= seen
            seen, loads = count, loads + 1
    stop.set()
    writer.join(60)
    assert stored and seen > 1


def test_a_database_without_a_store_is_refused_and_left_empty(place, refused):
    if place.backend == "sqlite":
        # A database of something else: SQLite's file has to be there.
        place.sql("CREATE TABLE notes (text TEXT)")
    tables = place.tables()
    refused(
        ValueError,
        f"{place.location} holds no QuantForge store; a reader creates none, and `quantforge "
        "data sync` does",
        qf.CandleReader,
        **place.arguments,
    )
    assert place.tables() == tables


VERSION = "SELECT value FROM meta WHERE key = 'schema_version'"


def test_a_store_at_another_schema_is_refused_and_not_migrated(stored, refused):
    current = int(stored.value(VERSION))
    before = fingerprint(stored)
    if stored.backend == "sqlite":
        forward = f"`quantforge --db {stored.location} store status`"
    else:
        forward = "`quantforge --storage postgres store status` with the same URL"
    # As an earlier build left it, and as a later one would.
    stored.sql(f"UPDATE meta SET value = '{current - 1}' WHERE key = 'schema_version'")
    refused(
        ValueError,
        f"{stored.location} is at schema {current - 1}, and this build reads schema {current}; a "
        "reader migrates nothing: bring the store forward with the quantforge command line of "
        f"this version ({qf.ENGINE_VERSION}), where any command does it in place, such as {forward}",
        qf.CandleReader,
        **stored.arguments,
    )
    assert stored.value(VERSION) == str(current - 1)
    stored.sql(f"UPDATE meta SET value = '{current + 1}' WHERE key = 'schema_version'")
    refused(
        ValueError,
        f"database schema version {current + 1} is newer than this build supports ({current}); "
        "use a newer quantforge release, and never an older one on this database",
        qf.CandleReader,
        **stored.arguments,
    )
    assert stored.value(VERSION) == str(current + 1)
    # Back at this build's schema, nothing else of it was touched.
    stored.sql(f"UPDATE meta SET value = '{current}' WHERE key = 'schema_version'")
    assert fingerprint(stored) == before
    with qf.CandleReader(**stored.arguments) as reader:
        assert len(reader.load(MARKET)) == 5


def test_a_store_an_earlier_build_wrote_is_read_once_a_command_brought_it_forward(
    tmp_path, repository
):
    # A store an earlier build's command line wrote, as the crate's own
    # tests keep one: three candles, at schema 9.
    place = stores.Place.sqlite(tmp_path)
    fixture = repository / "tests" / "fixtures" / "sqlite" / "schema-9.sql"
    connection = sqlite3.connect(place.path)
    connection.executescript(fixture.read_text(encoding="utf-8"))
    connection.close()
    before = place.path.read_bytes()
    with pytest.raises(ValueError) as caught:
        qf.CandleReader(**place.arguments)
    said = str(caught.value)
    assert said.startswith(f"{place.location} is at schema 9, and this build reads schema "), said
    command = f"`quantforge --db {place.location} store status`"
    assert said.endswith(f"where any command does it in place, such as {command}"), said
    assert place.path.read_bytes() == before
    # The command the refusal names, run as it is written.
    stores.quantforge("--db", place.location, "store", "status", cwd=tmp_path)
    with qf.CandleReader(**place.arguments) as reader:
        assert written(reader.load(MARKET)) == [
            (
                1792454400000,
                1792454459999,
                "67012.34000000",
                "67020.00000000",
                "67001.10000000",
                "67015.50000000",
                "12.04510000",
                1234,
            ),
            (
                1792540800000,
                1792540859999,
                "67100.00",
                "67100.00",
                "67100.00",
                "67100.00",
                "1.000",
                7,
            ),
            (
                1792540860000,
                1792540919999,
                "67100.00",
                "67105.50",
                "67099.90",
                "67101.10",
                "0.50000000",
                None,
            ),
        ]


def test_a_file_that_is_no_store_is_refused_as_what_it_is(tmp_path, refused):
    # No database at all.
    text = tmp_path / "notes.sqlite"
    text.write_text("not a database, and longer than a header\n", encoding="utf-8")
    refused(ValueError, f"{text} is not a SQLite database", qf.CandleReader, db=text)
    assert text.read_text(encoding="utf-8") == "not a database, and longer than a header\n"
    # A directory.
    refused(
        ValueError,
        f"{tmp_path} is a directory, and a SQLite database is a file",
        qf.CandleReader,
        db=tmp_path,
    )
    assert sorted(path.name for path in tmp_path.iterdir()) == ["notes.sqlite"]


def test_a_store_holding_an_unfinished_copy_is_refused_until_it_is_finished(
    tmp_path, postgres_server, refused
):
    with stores.Database(postgres_server) as database:
        place = stores.Place.postgres(database, tmp_path)
        place.store(five_minutes())
        own = place.value("SELECT store_id FROM store_control")
        source = "0123456789abcdef0123456789abcdef"
        record = (
            '{"format":1,"source_store_id":"%s","schema_version":%s,"state":"%s",'
            '"started_at_ms":%d,"copied_at_ms":%s}'
        )
        version = place.value(VERSION)
        copying = record % (source, version, "Copying", stores.NOON, "null")
        place.sql(f"INSERT INTO meta (key, value) VALUES ('transfer', '{copying}')")
        refused(
            ValueError,
            f"store {own} holds an unfinished copy of store {source}, begun 2026-01-01T12:00:00Z; "
            "finish it with `quantforge data migrate --from <the source>` before reading it",
            qf.CandleReader,
            **place.arguments,
        )
        copied = record % (source, version, "Copied", stores.NOON, stores.NOON + 1)
        place.sql(f"UPDATE meta SET value = '{copied}' WHERE key = 'transfer'")
        with qf.CandleReader(**place.arguments) as reader:
            assert len(reader.load(MARKET)) == 5


def test_a_server_that_is_not_reached_is_a_connection_error_that_names_no_user(
    tmp_path, monkeypatch, postgres_server
):
    monkeypatch.chdir(tmp_path)
    server = postgres_server
    where = f"postgres://{server.host}:{server.port}"
    sentinel = "reader-sentinel-word"
    cases = [
        # A database the server does not have.
        (
            server.of("qf_python_no_such_database").url,
            f"cannot open the postgres store at {where}/qf_python_no_such_database: storage "
            "unavailable: cannot get a connection: the database the URL names does not exist "
            "(SQLSTATE 3D000)",
        ),
        # A password it does not take.
        (
            server.parts._replace(
                netloc=f"{server.user}:{sentinel}@{server.host}:{server.port}"
            ).geturl(),
            f"cannot open the postgres store at {where}/{server.database}: storage unavailable: "
            "cannot get a connection: the server refused the URL's user or password (SQLSTATE "
            "28P01)",
        ),
    ]
    for url, said in cases:
        with pytest.raises(ConnectionError) as caught:
            qf.CandleReader(storage="postgres", postgres_url=url)
        assert type(caught.value) is ConnectionError and str(caught.value) == said
        assert sentinel not in said and server.secret not in said
    # With PostgreSQL selected, no SQLite file was made, opened or read.
    assert list(tmp_path.iterdir()) == []


def test_a_load_that_cannot_reach_its_store_is_a_connection_error_and_reads_again_once_it_can(
    tmp_path, postgres_server, refused
):
    with stores.Database(postgres_server) as database:
        place = stores.Place.postgres(database, tmp_path)
        place.store(five_minutes())
        with qf.CandleReader(**place.arguments) as reader:
            assert closes(reader.load(MARKET)) == CLOSES
            # The database stops taking connections, and the ones the reader
            # holds are ended: it cannot reach its store.
            admin = database.admin
            admin.query(f"ALTER DATABASE {database.name} ALLOW_CONNECTIONS false")
            admin.query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity "
                f"WHERE datname = '{database.name}'"
            )
            try:
                refused(
                    ConnectionError,
                    "storage unavailable: cannot get a connection: the server refused it "
                    "(SQLSTATE 55000)",
                    reader.load,
                    MARKET,
                )
            finally:
                admin.query(f"ALTER DATABASE {database.name} ALLOW_CONNECTIONS true")
            # Nothing was read and nothing changed: the same call reads once
            # the store is back, through the same reader.
            assert closes(reader.load(MARKET)) == CLOSES


def test_with_postgresql_selected_no_sqlite_file_is_made_and_a_user_is_never_shown(
    tmp_path, monkeypatch, postgres_server
):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "work").mkdir()
    with stores.Database(postgres_server) as database:
        place = stores.Place.postgres(database, tmp_path / "work")
        place.store(five_minutes())
        monkeypatch.chdir(tmp_path / "work")
        # By its variables alone, as the command line was told.
        for variable, value in place.variables.items():
            monkeypatch.setenv(variable, value)
        with qf.CandleReader() as reader:
            assert reader.backend == "postgres" and reader.location == place.location
            assert closes(reader.load(MARKET)) == CLOSES
            shown = f"{reader!r} {reader.location} {reader.backend}"
            assert postgres_server.secret not in shown
            assert f"{postgres_server.user}:" not in shown and "@" not in shown
    # Neither the command nor the reader made a SQLite file there.
    assert list((tmp_path / "work").iterdir()) == []


# How many of a reader's sessions wait on a lock another session holds.
WAITING = (
    "SELECT CAST(count(*) AS TEXT) FROM pg_stat_activity WHERE datname = "
    "current_database() AND application_name = 'quantforge' AND wait_event_type = 'Lock'"
)


def connections(place: stores.Place) -> int:
    """The connections a reader holds on a PostgreSQL database, by the name
    its sessions give themselves."""
    return int(
        place.value(
            "SELECT CAST(count(*) AS TEXT) FROM pg_stat_activity "
            "WHERE datname = current_database() AND application_name = 'quantforge'"
        )
    )


def eventually(found, expected, seconds: float = 10.0):
    """What `found()` says once it says `expected`, or last, after waiting."""
    deadline = time.monotonic() + seconds
    while (said := found()) != expected and time.monotonic() < deadline:
        time.sleep(0.02)
    return said


def test_a_reader_holds_its_pool_of_connections_and_no_more_and_closing_it_closes_them(
    tmp_path, postgres_server
):
    with stores.Database(postgres_server) as database:
        place = stores.Place.postgres(database, tmp_path)
        place.store(five_minutes())
        for size, pool in [(None, 2), (1, 1), (3, 3)]:
            arguments = dict(place.arguments)
            if size is not None:
                arguments["postgres_pool_size"] = size
            reader = qf.CandleReader(**arguments)
            loaded = []
            with database.server.session() as holder:
                # Every load waits on the table another session holds, each
                # on a connection of the pool's: one more load than the pool
                # has connections waits for a connection instead.
                holder.run("BEGIN")
                holder.run("LOCK TABLE candles IN ACCESS EXCLUSIVE MODE")
                threads = [
                    threading.Thread(target=lambda: loaded.append(reader.load(MARKET)), daemon=True)
                    for _ in range(pool + 1)
                ]
                for thread in threads:
                    thread.start()
                assert eventually(lambda: place.value(WAITING), str(pool), seconds=4) == str(pool)
                time.sleep(0.2)
                assert place.value(WAITING) == str(pool) and connections(place) == pool
                holder.run("COMMIT")
                for thread in threads:
                    thread.join(30)
            assert len(loaded) == pool + 1 and all(len(batch) == 5 for batch in loaded)
            reader.close()
            assert eventually(lambda: connections(place), 0) == 0
        # A reader that is no longer referenced closes its connections too.
        reader = qf.CandleReader(**place.arguments)
        assert len(reader.load(MARKET)) == 5 and connections(place) >= 1
        del reader
        gc.collect()
        assert eventually(lambda: connections(place), 0) == 0


def test_opening_a_reader_waits_without_the_interpreter(tmp_path, postgres_server):
    with stores.Database(postgres_server) as database:
        place = stores.Place.postgres(database, tmp_path)
        place.store(five_minutes())
        with database.server.session() as holder:
            # Another session holds the table a reader reads its schema's
            # version from, so opening one has to wait for it.
            holder.run("BEGIN")
            holder.run("LOCK TABLE meta IN ACCESS EXCLUSIVE MODE")
            opened = []
            opener = threading.Thread(
                target=lambda: opened.append(qf.CandleReader(**place.arguments)), daemon=True
            )
            opener.start()
            # This thread runs while that one waits on the server.
            assert eventually(lambda: place.value(WAITING), "1", seconds=4) == "1"
            assert opened == []
            holder.run("COMMIT")
            opener.join(30)
        (reader,) = opened
        with reader:
            assert closes(reader.load(MARKET)) == CLOSES


def test_a_load_waits_without_the_interpreter(tmp_path, postgres_server):
    with stores.Database(postgres_server) as database:
        place = stores.Place.postgres(database, tmp_path)
        place.store(five_minutes())
        waiting = WAITING
        with qf.CandleReader(**place.arguments) as reader, database.server.session() as holder:
            # Another session holds the table, so the load has to wait for it.
            holder.run("BEGIN")
            holder.run("LOCK TABLE candles IN ACCESS EXCLUSIVE MODE")
            loaded = []
            loader = threading.Thread(
                target=lambda: loaded.append(reader.load(MARKET)), daemon=True
            )
            loader.start()
            # This thread runs while that one waits on the server: it sees
            # the load waiting, and lets it go. Had the load kept the
            # interpreter, nothing here would run until it gave up.
            assert eventually(lambda: place.value(waiting), "1", seconds=4) == "1"
            assert loaded == []
            holder.run("COMMIT")
            loader.join(30)
        assert [closes(batch) for batch in loaded] == [CLOSES]
