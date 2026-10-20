//! Candles and kline requests: the shape of each candle, the window bounds
//! and limit, paging, and windows with no bars in them.

use rust_decimal::Decimal;

use super::support::{Anchor, Ctx, Fault, Verdict, ensure, same_candles};
use crate::{Candle, TimestampMs};

/// Bars in the window most checks read.
const WINDOW_BARS: i64 = 12;
/// Bars between the window's end and the anchor's bar, so that every bar
/// in the window has settled before it is read twice.
const SETTLED_BARS: i64 = 2;
/// The page size the paging check walks the window with.
const PAGE: u16 = 5;
/// More pages than a twelve-bar window can take at five a page, with room
/// for an empty last one.
const MAX_PAGES: usize = 8;
/// 2000-01-01T00:00:00Z, before any exchange listed anything.
const BEFORE_ANY_EXCHANGE: TimestampMs = 946_684_800_000;
/// How far past the anchor the future window opens, in bars.
const FUTURE_BARS: i64 = 1_000;

/// The window `[start, end]` of `WINDOW_BARS` settled bars before `anchor`.
fn window(ctx: &Ctx<'_>, anchor: &Anchor) -> (TimestampMs, TimestampMs) {
    let end = anchor
        .aligned
        .saturating_sub(SETTLED_BARS.saturating_mul(ctx.step));
    let start = end.saturating_sub((WINDOW_BARS - 1).saturating_mul(ctx.step));
    (start, end)
}

pub(super) async fn candle_normalization(ctx: &Ctx<'_>) -> Verdict {
    let rules = ctx.rules().await?;
    let tick = rules
        .tick_size
        .filter(|tick| *tick > Decimal::ZERO)
        .ok_or_else(|| Fault::Failed("the symbol rules report no price tick".to_string()))?;
    let anchor = ctx.anchor().await?;
    let (start, end) = window(ctx, &anchor);
    let candles = ctx.klines(Some(start), Some(end), None).await?;
    ensure!(
        !candles.is_empty(),
        "no candles between {start} and {end}, the {WINDOW_BARS} bars before the last one settled"
    );
    ctx.well_formed(&candles)?;
    for candle in &candles {
        let open = candle.open_time_ms;
        ensure!(
            (start..=end).contains(&open),
            "candle {open} lies outside the requested window {start} to {end}"
        );
        ensure!(
            candle.close_time_ms < anchor.server_ms,
            "candle {open} had not closed when the exchange clock read {}",
            anchor.server_ms
        );
        for (field, price) in [
            ("open", candle.open),
            ("high", candle.high),
            ("low", candle.low),
            ("close", candle.close),
        ] {
            ensure!(
                price.checked_rem(tick) == Some(Decimal::ZERO),
                "candle {open} has {field} {price}, off the {tick} tick grid; a price read \
                 through a float, or rounded, lands off it"
            );
        }
    }
    Ok(None)
}

pub(super) async fn kline_bounds(ctx: &Ctx<'_>) -> Verdict {
    let anchor = ctx.anchor().await?;
    let (start, end) = window(ctx, &anchor);
    let candles = ctx.klines(Some(start), Some(end), None).await?;
    ctx.well_formed(&candles)?;
    let [c0, c1, c2, ..] = candles.as_slice() else {
        return Err(Fault::Failed(format!(
            "only {} candles between {start} and {end}; the check needs 3",
            candles.len()
        )));
    };

    let bounded = ctx
        .klines(Some(c0.open_time_ms), Some(c2.open_time_ms), None)
        .await?;
    ensure!(
        same_candles(&bounded, &[c0.clone(), c1.clone(), c2.clone()]),
        "a window from {} to {} returned {} candles instead of the 3 opening there: both \
         bounds are inclusive on open time",
        c0.open_time_ms,
        c2.open_time_ms,
        bounded.len()
    );

    let first_two = ctx.klines(Some(c0.open_time_ms), None, Some(2)).await?;
    ensure!(
        same_candles(&first_two, &[c0.clone(), c1.clone()]),
        "a start of {} with limit 2 returned {} candles instead of the first 2 from there",
        c0.open_time_ms,
        first_two.len()
    );

    let capped = ctx.klines(Some(start), Some(end), Some(u16::MAX)).await?;
    ensure!(
        same_candles(&capped, &candles),
        "a limit of {} must be capped to the declared maximum of {} and served like the window \
         without one",
        u16::MAX,
        ctx.capabilities.market_data.max_klines_per_request
    );
    Ok(None)
}

pub(super) async fn kline_paging(ctx: &Ctx<'_>) -> Verdict {
    let anchor = ctx.anchor().await?;
    let (start, end) = window(ctx, &anchor);
    let whole = ctx.klines(Some(start), Some(end), None).await?;
    ctx.well_formed(&whole)?;

    // Walked the way data sync walks a range.
    let mut paged: Vec<Candle> = Vec::new();
    let mut cursor = start;
    let mut pages = 0;
    while cursor <= end {
        pages += 1;
        ensure!(
            pages <= MAX_PAGES,
            "paging {start} to {end} did not finish within {MAX_PAGES} pages"
        );
        let page = ctx.klines(Some(cursor), Some(end), Some(PAGE)).await?;
        ensure!(
            page.len() <= usize::from(PAGE),
            "a page of {PAGE} returned {} candles",
            page.len()
        );
        let Some(last) = page.last() else { break };
        let next = last.open_time_ms.saturating_add(ctx.step);
        ensure!(
            next > cursor,
            "the page from {cursor} did not advance: its last candle opens at {}",
            last.open_time_ms
        );
        cursor = next;
        paged.extend(page);
    }
    ensure!(
        same_candles(&paged, &whole),
        "pages of {PAGE} returned {} candles that differ from the {} of one request over the \
         same window",
        paged.len(),
        whole.len()
    );
    Ok(None)
}

pub(super) async fn kline_gaps(ctx: &Ctx<'_>) -> Verdict {
    let anchor = ctx.anchor().await?;
    let ten_bars = ctx.step.saturating_mul(10);

    let future_start = anchor
        .aligned
        .saturating_add(FUTURE_BARS.saturating_mul(ctx.step));
    let future = ctx
        .klines(
            Some(future_start),
            Some(future_start.saturating_add(ten_bars)),
            None,
        )
        .await?;
    ensure!(
        future.is_empty(),
        "a window {FUTURE_BARS} bars ahead returned {} candles",
        future.len()
    );

    let ancient = ctx
        .klines(
            Some(BEFORE_ANY_EXCHANGE),
            Some(BEFORE_ANY_EXCHANGE.saturating_add(ten_bars)),
            None,
        )
        .await?;
    ensure!(
        ancient.is_empty(),
        "a window in 2000 returned {} candles",
        ancient.len()
    );

    let (_, end) = window(ctx, &anchor);
    let oldest = ctx
        .klines(Some(BEFORE_ANY_EXCHANGE), Some(end), Some(3))
        .await?;
    ensure!(
        !oldest.is_empty(),
        "a window opening in 2000 returned no candles: a leading gap must be skipped to the \
         oldest bars, or a sync from an early start stops at once"
    );
    ensure!(
        oldest.len() <= 3,
        "a limit of 3 returned {} candles",
        oldest.len()
    );
    ctx.well_formed(&oldest)?;
    for candle in &oldest {
        ensure!(
            (BEFORE_ANY_EXCHANGE..=end).contains(&candle.open_time_ms),
            "candle {} lies outside the requested window {BEFORE_ANY_EXCHANGE} to {end}",
            candle.open_time_ms
        );
    }
    Ok(None)
}
