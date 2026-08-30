//! Epoch-millisecond timestamps and their RFC 3339 conversions. Every
//! timestamp in the crate is UTC epoch milliseconds.

use super::ModelError;
use time::OffsetDateTime;

pub type TimestampMs = i64;

pub fn parse_rfc3339_to_ms(input: &str) -> Result<TimestampMs, ModelError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ModelError::InvalidTimestamp("empty".to_string()));
    }
    let dt = OffsetDateTime::parse(input, &time::format_description::well_known::Rfc3339)?;
    Ok(dt.unix_timestamp() * 1000 + i64::from(dt.millisecond()))
}

/// Formats epoch milliseconds as an RFC 3339 timestamp for display.
///
/// Values outside the representable datetime range render as an explicit
/// `invalid-ms(<value>)` marker instead of silently falling back to a
/// plausible-looking epoch date.
pub fn ms_to_rfc3339(ms: TimestampMs) -> String {
    let seconds = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000) as u16;

    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|dt| dt.replace_millisecond(millis).ok())
        .and_then(|dt| {
            dt.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| format!("invalid-ms({ms})"))
}

pub fn now_utc_ms() -> TimestampMs {
    let now = OffsetDateTime::now_utc();
    now.unix_timestamp() * 1000 + i64::from(now.millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_to_rfc3339_formats_valid_timestamps() {
        assert_eq!(ms_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(ms_to_rfc3339(1_500), "1970-01-01T00:00:01.5Z");
    }

    #[test]
    fn ms_to_rfc3339_marks_out_of_range_timestamps_instead_of_epoch() {
        let rendered = ms_to_rfc3339(i64::MAX);
        assert_eq!(rendered, format!("invalid-ms({})", i64::MAX));
        let rendered = ms_to_rfc3339(i64::MIN);
        assert_eq!(rendered, format!("invalid-ms({})", i64::MIN));
    }
}
