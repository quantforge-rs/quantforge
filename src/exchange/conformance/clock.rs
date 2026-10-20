//! The exchange clock, and the closed-bar semantics measured against it.

use super::Environment;
use super::support::{Ctx, Fault, Verdict, ensure};
use crate::{TimestampMs, now_utc_ms};

/// Readings outside this range are not epoch milliseconds at all, but
/// seconds or microseconds read as milliseconds: 2001-09-09 to 2286.
const PLAUSIBLE_MS: std::ops::Range<TimestampMs> = 1_000_000_000_000..10_000_000_000_000;

/// Drift past this between the local clock and the exchange's makes signed
/// requests unreliable, whatever the connector does.
const MAX_DRIFT_MS: TimestampMs = 1_000;

/// How many of the latest candles `closed_bars` asks for.
const LATEST: u16 = 3;

pub(super) async fn server_time(ctx: &Ctx<'_>) -> Verdict {
    // The local readings shape no request: replayed, they are ignored.
    let before = now_utc_ms();
    let first = ctx.server_time().await?;
    let after = now_utc_ms();
    let second = ctx.server_time().await?;
    ensure!(
        PLAUSIBLE_MS.contains(&first),
        "the exchange clock read {first}, which is not UTC epoch milliseconds"
    );
    ensure!(
        second >= first,
        "the exchange clock ran backwards, from {first} to {second}"
    );
    if ctx.target.environment == Environment::Replay {
        return Ok(None);
    }

    let local = before.saturating_add(after.saturating_sub(before) / 2);
    let drift = first.saturating_sub(local);
    if drift.saturating_abs() > MAX_DRIFT_MS {
        return Err(Fault::Inconclusive(format!(
            "the local clock is {drift:+} ms off the exchange's; signed requests are unreliable \
             until it is synchronized"
        )));
    }
    Ok(Some(format!("local clock drift {drift:+} ms")))
}

pub(super) async fn closed_bars(ctx: &Ctx<'_>) -> Verdict {
    let t0 = ctx.server_time().await?;
    let latest = ctx.klines(None, None, Some(LATEST)).await?;
    let t1 = ctx.server_time().await?;
    ctx.well_formed(&latest)?;
    let Some((newest, older)) = latest.split_last() else {
        return Err(Fault::Failed(
            "a request for the latest candles returned none".to_string(),
        ));
    };
    ensure!(
        latest.len() <= usize::from(LATEST),
        "a request for the latest {LATEST} candles returned {}",
        latest.len()
    );
    ensure!(
        newest.open_time_ms <= t1,
        "candle {} opens after the exchange clock read {t1}",
        newest.open_time_ms
    );
    for candle in older {
        ensure!(
            candle.close_time_ms < t1,
            "candle {} is not the newest, yet it was still open when the exchange clock read {t1}",
            candle.open_time_ms
        );
    }
    // A quiet market may have no bar forming at all.
    Ok((newest.close_time_ms < t0).then(|| "no bar was forming".to_string()))
}
