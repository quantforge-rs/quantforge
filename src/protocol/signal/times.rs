//! A signal's times: text as TradingView renders it, read strictly.

use std::fmt;

/// The length of a time's text: `yyyy-MM-ddTHH:mm:ssZ`.
const LENGTH: usize = 20;

/// The first year a time may name.
const FIRST_YEAR: i64 = 1970;

/// A time of a signal: UTC, to the second, written exactly as TradingView
/// renders `{{time}}` and `{{timenow}}`, `yyyy-MM-ddTHH:mm:ssZ`, such as
/// `2026-10-20T00:15:00Z`. One instant has one text: no offset, no
/// fraction, no lowercase, and no year before 1970 or after 9999.
///
/// Times compare by the instant they name.
///
/// ```
/// use quantforge::protocol::signal::Signal;
///
/// let signal = Signal::read(
///     br#"{"version":1,"kind":"exit","profile":"main","source":"trend",
///         "ticker":"SOLUSDT.P","side":"long",
///         "bar_time":"2026-10-20T00:00:00Z","fire_time":"2026-10-20T00:15:00Z"}"#,
/// )?;
/// assert_eq!(signal.fire_time().as_str(), "2026-10-20T00:15:00Z");
/// assert_eq!(signal.fire_time().ms(), 1_792_455_300_000);
/// assert!(signal.bar_time() < signal.fire_time());
/// # Ok::<(), quantforge::protocol::signal::SignalProblems>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignalTime {
    // Times order by their instant. A time's text, every part of it at a
    // fixed place and width, orders the same way.
    ms: i64,
    text: String,
}

impl SignalTime {
    /// Reads a time's text, or `None` for anything but the one form.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != LENGTH {
            return None;
        }
        let number = |from: usize, to: usize| {
            bytes[from..to].iter().try_fold(0i64, |value, byte| {
                byte.is_ascii_digit()
                    .then(|| value * 10 + i64::from(byte - b'0'))
            })
        };
        let punctuated = [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'Z'),
        ]
        .iter()
        .all(|(at, mark)| bytes[*at] == *mark);
        if !punctuated {
            return None;
        }
        let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
        let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
        let real = year >= FIRST_YEAR
            && (1..=12).contains(&month)
            && (1..=days_in(year, month)).contains(&day)
            && hour <= 23
            && minute <= 59
            && second <= 59;
        if !real {
            return None;
        }
        let seconds =
            days_from_epoch(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
        Some(Self {
            ms: seconds * 1_000,
            text: text.to_string(),
        })
    }

    /// The time as it was written.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The instant, in milliseconds since the Unix epoch.
    pub fn ms(&self) -> i64 {
        self.ms
    }
}

/// The time as it was written.
impl fmt::Display for SignalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in(year: i64, month: i64) -> i64 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 31,
    }
}

/// The days from 1970-01-01 to a date of the Gregorian calendar, by the
/// usual count of whole 400-year eras from a year that starts in March.
fn days_from_epoch(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(text: &str) -> Option<i64> {
        SignalTime::parse(text).map(|time| time.ms())
    }

    #[test]
    fn a_time_is_the_instant_its_text_names() {
        assert_eq!(ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(ms("1970-01-01T00:00:01Z"), Some(1_000));
        assert_eq!(ms("1970-12-31T23:59:59Z"), Some(31_535_999_000));
        assert_eq!(ms("2000-02-29T12:00:00Z"), Some(951_825_600_000));
        assert_eq!(ms("2026-10-20T00:00:00Z"), Some(1_792_454_400_000));
        assert_eq!(ms("2026-10-20T00:15:00Z"), Some(1_792_455_300_000));
        assert_eq!(ms("2028-02-29T23:59:59Z"), Some(1_835_481_599_000));
        assert_eq!(ms("2028-03-01T00:00:00Z"), Some(1_835_481_600_000));
        assert_eq!(ms("9999-12-31T23:59:59Z"), Some(253_402_300_799_000));
    }

    #[test]
    fn every_day_follows_the_one_before_it() {
        // Four centuries of days, each a day's milliseconds after the last.
        let mut expected = 0;
        for year in 1970..2371 {
            for month in 1..=12 {
                for day in 1..=days_in(year, month) {
                    let text = format!("{year:04}-{month:02}-{day:02}T00:00:00Z");
                    assert_eq!(ms(&text), Some(expected), "{text}");
                    expected += 86_400_000;
                }
            }
        }
    }

    #[test]
    fn anything_but_the_one_form_is_no_time() {
        for text in [
            "",
            "2026-10-20",
            "2026-10-20T00:15:00",
            "2026-10-20T00:15:00z",
            "2026-10-20t00:15:00Z",
            "2026-10-20 00:15:00Z",
            "2026-10-20T00:15:00.000Z",
            "2026-10-20T00:15:00+00:00",
            "2026-10-20T02:15:00+02:00",
            "2026-10-20T00:15Z",
            "26-10-20T00:15:00Z",
            "2026-1-20T00:15:00Z",
            "2026/10/20T00:15:00Z",
            "2026-10-20T00.15.00Z",
            " 2026-10-20T00:15:00Z",
            "2026-10-20T00:15:00Z ",
            "+026-10-20T00:15:00Z",
            "2026-10-20T-1:15:00Z",
            "1792455300000",
            "{{timenow}}",
        ] {
            assert_eq!(ms(text), None, "{text:?}");
        }
        // Twenty bytes that are not twenty characters.
        assert_eq!(ms("2026-10-20T00:15:0\u{e9}"), None);
    }

    #[test]
    fn a_time_the_calendar_or_the_clock_lacks_is_no_time() {
        for text in [
            "1969-12-31T23:59:59Z",
            "0000-01-01T00:00:00Z",
            "2026-00-10T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-10-00T00:00:00Z",
            "2026-10-32T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "2026-10-20T24:00:00Z",
            "2026-10-20T00:60:00Z",
            "2026-10-20T00:00:60Z",
            "2026-12-31T23:59:60Z",
        ] {
            assert_eq!(ms(text), None, "{text}");
        }
        for text in [
            "2024-02-29T00:00:00Z",
            "2400-02-29T00:00:00Z",
            "2026-10-31T23:59:59Z",
        ] {
            assert!(ms(text).is_some(), "{text}");
        }
    }

    #[test]
    fn times_order_by_their_instant_and_keep_their_text() {
        let earlier = SignalTime::parse("2026-10-20T00:00:00Z").expect("time");
        let later = SignalTime::parse("2026-10-20T00:15:00Z").expect("time");
        assert!(earlier < later);
        assert_eq!(later.to_string(), "2026-10-20T00:15:00Z");
        assert_eq!(later.as_str(), "2026-10-20T00:15:00Z");
    }
}
