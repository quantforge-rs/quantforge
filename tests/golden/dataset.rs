//! How the golden candles were made, and what they were made to hold.
//!
//! No exchange's data is in the dataset. Every number comes from the
//! generator written out here, so the file can be made again from this
//! module alone, on any machine: no clock and no random source has a part
//! in it. The candles are shaped as Binance writes a Spot kline all the
//! same, since that is what a store holds after `data sync`: prices and
//! volumes with eight decimal places, a trade count on every bar, and a bar
//! in which nothing traded standing at the close before it with a volume of
//! zero.
//!
//! The file is the dataset, and this module is its provenance. The file is
//! frozen: a change here that made other candles would fail
//! `the_generator_makes_the_frozen_candles`, and would be a change to the
//! generator to undo, never a reason to write the file again.

use super::{CANDLES, candles, csv, read};
use quantforge::{Candle, TimestampMs};
use rust_decimal::Decimal;
use std::ops::Range;

/// Noon on 2026-01-01, UTC. The day runs to noon on the 2nd, so that its
/// candles lie in two of a store's one-day windows: the first half is kept
/// as a packed segment, and the second as the bars not packed yet.
pub(crate) const FIRST_OPEN_MS: TimestampMs = 1_767_268_800_000;

/// Twenty-four hours of one-minute bars.
pub(crate) const BARS: usize = 1_440;

const MINUTE_MS: TimestampMs = 60_000;
const DAY_MS: TimestampMs = 86_400_000;

/// How many bars the market stands still for, each time.
pub(crate) const STILL_BARS: usize = 60;

/// The two hours in which nothing trades, by the places of their bars: the
/// first comes at the end of a rise, which the strategy is long through, and
/// the second at the end of a fall, which it is flat through.
pub(crate) const STILL: [Range<usize>; 2] = [392..452, 1_098..1_158];

/// The turns of the price after which the market stands still: the seventh,
/// a rise, and the eighteenth, a fall.
const STILL_AFTER: [u32; 2] = [7, 18];

/// How many bars the quiet stretch lasts: an hour and a half, so that from
/// its forty-ninth bar on, for forty-two bars, the slow average holds its
/// two prices alone.
pub(crate) const QUIET_BARS: usize = 90;

/// The stretch in which the price only moves between two prices a tick
/// apart, by the places of its bars.
pub(crate) const QUIET: Range<usize> = 766..856;

/// The turn of the price after which the quiet stretch comes.
const QUIET_AFTER: u32 = 13;

/// The generator's seed: the first, counting from one, whose candles hold
/// everything `the_goldens_hold_what_the_dataset_was_made_to_show` states
/// and tell apart every strategy the Python package's tests hold up
/// against them.
const SEED: u64 = 50;

/// A linear congruential generator (Knuth's MMIX constants), read from its
/// upper bits.
struct Generator(u64);

impl Generator {
    /// The next number, from zero up to `limit` and not including it.
    fn below(&mut self, limit: i64) -> i64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        i64::try_from((self.0 >> 33) % limit.unsigned_abs()).expect("a number below the limit")
    }
}

/// What the market is doing over a run of bars.
#[derive(Clone, Copy)]
enum Market {
    /// The price rises or falls, under noise.
    Turning,
    /// Nothing trades.
    Still,
    /// The price moves between two prices a tick apart.
    Quiet,
}

/// A price of so many cents, written with eight decimal places.
fn price(cents: i64) -> Decimal {
    Decimal::new(cents * 1_000_000, 8)
}

/// The candles: a price that rises and falls by turns under noise of its
/// own, twice stands still for an hour, and once is quiet for an hour and
/// a half.
///
/// - **A turn** lasts 20 to 79 bars and moves the price by 6.00 to 17.99 a
///   bar, up on the odd turns and down on the even ones, the first a rise.
///   Each bar opens within 1.00 of the close before it, closes its turn's
///   slope away from its open give or take 30.00, and has wicks of less
///   than 15.00, a volume of up to nine and a trade count of up to five
///   thousand.
/// - **In a still hour** every bar stands at the close before it, with no
///   volume and no trade.
/// - **In the quiet stretch** every bar opens where the one before it
///   closed and closes at one of two prices a tick apart, the close before
///   the stretch and a cent above it, on a volume of up to a half and up to
///   fifty trades.
pub(crate) fn generated() -> Vec<Candle> {
    let mut random = Generator(SEED);
    // In cents: 82,500.00.
    let mut last_close: i64 = 8_250_000;
    let (mut turn, mut left, mut slope) = (0_u32, 0_i64, 0_i64);
    let mut market = Market::Turning;
    let mut stood = [false; STILL_AFTER.len()];
    let mut quieted = false;
    // The lower of the quiet stretch's two prices.
    let mut floor = 0_i64;
    (0..BARS)
        .map(|index| {
            if left == 0 {
                let due = STILL_AFTER.iter().position(|after| *after == turn);
                if let Some(hour) = due.filter(|hour| !stood[*hour]) {
                    stood[hour] = true;
                    market = Market::Still;
                    left = STILL_BARS as i64;
                } else if turn == QUIET_AFTER && !quieted {
                    quieted = true;
                    market = Market::Quiet;
                    floor = last_close;
                    left = QUIET_BARS as i64;
                } else {
                    turn += 1;
                    market = Market::Turning;
                    let steep = 600 + random.below(1_200);
                    slope = if turn % 2 == 1 { steep } else { -steep };
                    left = 20 + random.below(60);
                }
            }
            left -= 1;
            let open_time_ms = FIRST_OPEN_MS + index as TimestampMs * MINUTE_MS;
            let close_time_ms = open_time_ms + MINUTE_MS - 1;
            match market {
                Market::Still => Candle {
                    open_time_ms,
                    close_time_ms,
                    open: price(last_close),
                    high: price(last_close),
                    low: price(last_close),
                    close: price(last_close),
                    volume: Decimal::new(0, 8),
                    trades: Some(0),
                },
                Market::Quiet => {
                    let open = last_close;
                    let close = floor + random.below(2);
                    last_close = close;
                    Candle {
                        open_time_ms,
                        close_time_ms,
                        open: price(open),
                        high: price(open.max(close)),
                        low: price(open.min(close)),
                        close: price(close),
                        volume: Decimal::new(random.below(50_000_000) + 1, 8),
                        trades: Some((random.below(50) + 1).unsigned_abs()),
                    }
                }
                Market::Turning => {
                    let open = last_close + random.below(201) - 100;
                    let close = open + slope + random.below(6_001) - 3_000;
                    let high = open.max(close) + random.below(1_500);
                    let low = open.min(close) - random.below(1_500);
                    last_close = close;
                    Candle {
                        open_time_ms,
                        close_time_ms,
                        open: price(open),
                        high: price(high),
                        low: price(low),
                        close: price(close),
                        volume: Decimal::new(random.below(900_000_000) + 1, 8),
                        trades: Some((random.below(5_000) + 1).unsigned_abs()),
                    }
                }
            }
        })
        .collect()
}

#[test]
fn the_generator_makes_the_frozen_candles() {
    let made = generated();
    assert_eq!(made, generated(), "the same candles on every run");
    let (written, frozen) = (csv(&made), read(CANDLES));
    assert!(
        written == frozen,
        "the generator no longer makes {CANDLES}, first at line {}: the file is frozen, so the \
         change to undo is the generator's",
        super::first_difference(&frozen, &written)
    );
    // The generator is written out, and so are its first numbers.
    let first = made.first().expect("a first candle");
    assert_eq!(
        csv(std::slice::from_ref(first)).lines().nth(1),
        Some(
            "1767268800000,1767268859999,82499.72000000,82522.43000000,82499.35000000,\
             82516.20000000,8.61711596,1076"
        )
    );
}

#[test]
fn the_candles_are_a_day_of_minutes_from_noon_to_noon() {
    let candles = candles();
    assert_eq!(candles.len(), BARS);
    for (index, candle) in candles.iter().enumerate() {
        let opens = FIRST_OPEN_MS + index as TimestampMs * MINUTE_MS;
        assert_eq!(
            (candle.open_time_ms, candle.close_time_ms),
            (opens, opens + MINUTE_MS - 1),
            "bar {index}"
        );
    }
    // Half of them on each of two days, UTC: a store's window of one-minute
    // candles is a day, and the dataset lies in two.
    assert_eq!(FIRST_OPEN_MS % DAY_MS, DAY_MS / 2);
    let day = |candle: &Candle| candle.open_time_ms.div_euclid(DAY_MS);
    let first_day = day(&candles[0]);
    let on_the_first = candles
        .iter()
        .filter(|candle| day(candle) == first_day)
        .count();
    assert_eq!(on_the_first, BARS / 2);
    assert!(
        candles[BARS / 2..]
            .iter()
            .all(|candle| day(candle) == first_day + 1)
    );
}

#[test]
fn the_candles_are_written_as_the_exchange_writes_them() {
    let cent = Decimal::new(1, 2);
    for candle in candles() {
        // Prices of whole cents, the market's tick, with eight decimal
        // places; a volume of eight places; and a trade count on every bar.
        for value in [candle.open, candle.high, candle.low, candle.close] {
            assert_eq!(value.scale(), 8, "{value}");
            assert_eq!(value % cent, Decimal::ZERO, "{value}");
            assert!(
                value > Decimal::from(80_000) && value < Decimal::from(90_000),
                "{value}"
            );
        }
        assert_eq!(candle.volume.scale(), 8, "{}", candle.volume);
        assert!(candle.trades.is_some());
        assert_eq!(candle.volume.is_zero(), candle.trades == Some(0));
    }
    // The text keeps the places a zero has: a store or a reader that dropped
    // them would hand over other candles.
    assert!(read(CANDLES).contains(",0.00000000,0\n"));
}

#[test]
fn twice_the_market_stands_still_for_an_hour() {
    let candles = candles();
    let still = |index: usize| STILL.iter().any(|hour| hour.contains(&index));
    for hour in STILL {
        assert_eq!(hour.len(), STILL_BARS);
        // Each bar of the hour stands at the close before the hour.
        let before = candles[hour.start - 1].close;
        for candle in &candles[hour] {
            let prices = [candle.open, candle.high, candle.low, candle.close];
            assert!(prices.iter().all(|value| *value == before), "{candle:?}");
        }
    }
    // No bar outside the two hours is without a trade, and outside them and
    // the quiet stretch none stands where the bar before it closed.
    for (index, candle) in candles.iter().enumerate() {
        assert_eq!(candle.volume.is_zero(), still(index), "bar {index}");
        if !still(index) && !QUIET.contains(&index) {
            assert!(candle.high > candle.low, "bar {index}");
        }
    }
}

#[test]
fn for_an_hour_and_a_half_the_price_moves_between_two_ticks() {
    let candles = candles();
    assert_eq!(QUIET.len(), QUIET_BARS);
    assert!(
        STILL
            .iter()
            .all(|hour| hour.end <= QUIET.start || QUIET.end <= hour.start)
    );
    let floor = candles[QUIET.start - 1].close;
    let ceiling = floor + Decimal::new(1, 2);
    let half = Decimal::new(5, 1);
    for index in QUIET {
        let candle = &candles[index];
        // It opens where the bar before it closed, closes at one of the two
        // prices, and went nowhere else between.
        assert_eq!(candle.open, candles[index - 1].close, "bar {index}");
        assert!(
            candle.close == floor || candle.close == ceiling,
            "bar {index}"
        );
        assert_eq!(candle.high, candle.open.max(candle.close), "bar {index}");
        assert_eq!(candle.low, candle.open.min(candle.close), "bar {index}");
        // On little volume, and not on none.
        assert!(
            candle.volume > Decimal::ZERO && candle.volume <= half,
            "bar {index}"
        );
        assert!(matches!(candle.trades, Some(1..=50)), "bar {index}");
    }
    // Both prices are closed at, each on a good part of the bars.
    let at = |level: Decimal| QUIET.filter(|index| candles[*index].close == level).count();
    assert_eq!(at(floor) + at(ceiling), QUIET_BARS);
    assert!(at(floor) > QUIET_BARS / 3 && at(ceiling) > QUIET_BARS / 3);
}
