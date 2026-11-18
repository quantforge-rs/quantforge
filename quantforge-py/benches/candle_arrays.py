"""How long a batch of candles takes to cross to NumPy arrays and back.

    python benches/candle_arrays.py [rows]

It times `CandleBatch.from_numpy` and `CandleBatch.to_numpy` on `rows`
one-minute candles, a million unless told otherwise, with prices to the cent
and volumes to eight places as an exchange reports them, and prints the best
of five runs of each. For scale it also times the same columns read as tuples
of `Decimal` and turned into arrays in Python.

The figures are information, not a test: nothing holds a run to them. Measure
an optimized build, `maturin develop --release --extras numpy`, which installs
NumPy with the package; the build `maturin develop` makes without `--release`
is several times slower.
"""

import sys
import time

import numpy as np

import quantforge_py as qf

DECIMALS = ["open", "high", "low", "close", "volume"]
RUNS = 5


def columns(rows: int) -> dict:
    """`rows` one-minute candles as arrays, the same on every run: a price
    that wanders to the cent, a volume to eight places, and a trade count
    that every thousandth candle lacks."""
    random = np.random.default_rng(475)
    opened = np.arange(rows, dtype=np.int64) * 60_000 + 1_700_000_000_000
    close = np.round(81_940 + np.cumsum(random.normal(0, 12, rows)), 2)
    open = np.concatenate(([81_940.0], close[:-1]))
    spread = np.round(np.abs(random.normal(0, 9, rows)), 2)
    counts = random.integers(0, 5_000, rows, dtype=np.uint64)
    return dict(
        open_time_ms=opened,
        close_time_ms=opened + 59_999,
        open=open,
        high=np.round(np.maximum(open, close) + spread, 2),
        low=np.round(np.minimum(open, close) - spread, 2),
        close=close,
        volume=np.round(random.gamma(2.0, 3.0, rows), 8),
        trades=np.ma.MaskedArray(counts, mask=np.arange(rows) % 1_000 == 999),
    )


def best(call) -> tuple[float, object]:
    """The shortest of a few runs of `call`, in seconds, and what it gave."""
    shortest, given = float("inf"), None
    for _ in range(RUNS):
        started = time.perf_counter()
        given = call()
        shortest = min(shortest, time.perf_counter() - started)
    return shortest, given


def said(what: str, seconds: float, rows: int) -> None:
    print(f"{what}: {seconds * 1000:.1f} ms, {rows / seconds / 1e6:.2f} million rows a second")


def main() -> None:
    rows = int(sys.argv[1]) if len(sys.argv) > 1 else 1_000_000
    market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
    given = columns(rows)
    print(f"rows: {rows}")
    print(f"python {sys.version.split()[0]}, numpy {np.__version__}, engine {qf.ENGINE_VERSION}")

    taken, batch = best(lambda: qf.CandleBatch.from_numpy(**given, market=market))
    said("from_numpy", taken, rows)
    taken, arrays = best(batch.to_numpy)
    said("to_numpy", taken, rows)

    # What crossed is what was given: a float with at most fifteen digits
    # comes back the same float, and a count that was masked comes back
    # masked.
    for name, array in given.items():
        if name != "trades":
            assert np.array_equal(arrays[name], array), name
    assert np.array_equal(arrays["trades"].mask, array.mask)
    assert np.array_equal(arrays["trades"].compressed(), array.compressed())

    def in_python():
        return {
            name: np.fromiter(map(float, getattr(batch, name)), dtype=np.float64, count=rows)
            for name in DECIMALS
        }

    taken, slow = best(in_python)
    said("the five decimal columns through tuples of Decimal, in Python", taken, rows)
    for name in DECIMALS:
        assert np.array_equal(slow[name], arrays[name]), name


if __name__ == "__main__":
    main()
