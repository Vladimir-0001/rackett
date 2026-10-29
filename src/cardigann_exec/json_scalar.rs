//! Json.NET 13.0.4's default ISO date token conversion, followed by the invariant
//! Value<string>() used by Cardigann's JSON field selector. Boolean casing lives
//! beside the other scalar conversions in selector.rs. This is not JSON rewriting:
//! only selected scalar values are converted; keys and raw payloads stay intact.
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

fn digits(s: &[u8], from: usize, count: usize) -> Option<u32> {
    s.get(from..from + count)?.iter().try_fold(0, |n, b| {
        b.is_ascii_digit().then(|| n * 10 + u32::from(*b - b'0'))
    })
}

/// ISO recognition follows DateTimeParser.cs and DateTimeUtils.cs at the pinned
/// Json.NET version. Invalid ISO-looking strings remain strings, as upstream.
/// UTC/unspecified values retain their clock; explicit offsets become local time
/// under RoundtripKind. Fractions disappear in invariant DateTime.ToString().
/// Microsoft /Date(...)/ tokens remain outside this ISO slice.
pub(super) fn date_text(s: &str) -> Result<Option<String>, String> {
    let b = s.as_bytes();
    if !(19..=40).contains(&b.len()) || !b.is_ascii() {
        return Ok(None);
    }
    let parsed = (|| {
        if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
            return None;
        }
        let year = digits(b, 0, 4)?;
        if year == 0 {
            return None;
        }
        let day = NaiveDate::from_ymd_opt(year as i32, digits(b, 5, 2)?, digits(b, 8, 2)?)?;
        let hour = digits(b, 11, 2)?;
        let minute = digits(b, 14, 2)?;
        let second = digits(b, 17, 2)?;
        if hour > 24 || minute > 59 || second > 59 || (hour == 24 && (minute != 0 || second != 0)) {
            return None;
        }
        let mut at = 19;
        let mut fraction = 0;
        if b.get(at) == Some(&b'.') {
            at += 1;
            let start = at;
            while at < b.len() && at - start < 7 && b[at].is_ascii_digit() {
                fraction = fraction * 10 + u32::from(b[at] - b'0');
                at += 1;
            }
            if at == start || (hour == 24 && fraction != 0) {
                return None;
            }
            fraction *= 10u32.pow(7 - (at - start) as u32);
        }
        // Keep Json.NET's actual accepted zone syntax, including +HH, +HHmm,
        // +HH:mm and the historical trailing-colon forms. It accepts 00..99
        // zone components and normalizes them through TimeSpan, not FixedOffset.
        let mut direction = 0i64;
        let mut zone_hour = 0;
        let mut zone_minute = 0;
        if let Some(ch) = b.get(at) {
            if matches!(ch, b'Z' | b'z') {
                at += 1;
            } else {
                if let Some(h) = digits(b, at + 1, 2) {
                    zone_hour = h;
                    if matches!(ch, b'+' | b'-') {
                        direction = if *ch == b'+' { 1 } else { -1 };
                        at += 3;
                    }
                }
                if at < b.len() {
                    if b[at] == b':' {
                        at += 1;
                    }
                    if let Some(m) = digits(b, at, 2) {
                        zone_minute = m;
                        at += 2;
                    }
                }
            }
        }
        if at != b.len() {
            return None;
        }
        Some((
            day,
            hour,
            minute,
            second,
            fraction,
            direction,
            zone_hour,
            zone_minute,
        ))
    })();
    let Some((day, hour, minute, second, fraction, direction, zone_hour, zone_minute)) = parsed
    else {
        return Ok(None);
    };
    let failure = || "JSON date exceeds supported DateTime range".to_owned();
    let mut date = day
        .and_hms_nano_opt(hour.min(23), minute, second, fraction * 100)
        .ok_or_else(failure)?;
    if hour == 24 {
        date = day
            .succ_opt()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .ok_or_else(failure)?;
    }
    if direction != 0 {
        let seconds = direction * i64::from(zone_hour * 3600 + zone_minute * 60);
        let utc = date
            .checked_sub_signed(Duration::seconds(seconds))
            .ok_or_else(failure)?;
        // Match Json.NET RoundtripKind's explicit-offset => local DateTime path.
        // Overflow clamping at year 1/9999 is deliberately rejected instead.
        if !(1..=9999).contains(&utc.year()) {
            return Err(failure());
        }
        date = Utc
            .from_utc_datetime(&utc)
            .with_timezone(&Local)
            .naive_local();
    }
    format_date(date).map(Some)
}
fn format_date(date: NaiveDateTime) -> Result<String, String> {
    if !(1..=9999).contains(&date.year()) {
        return Err("JSON date exceeds supported DateTime range".into());
    }
    Ok(date.format("%m/%d/%Y %H:%M:%S").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn iso_dates_follow_pinned_recognition_and_invariant_scalar_conversion() {
        for input in [
            "2024-02-29T01:02:03",
            "2024-02-29T01:02:03Z",
            "2024-02-29T01:02:03z",
            "2024-02-29T01:02:03.1Z",
            "2024-02-29T01:02:03.1234567Z",
            "2024-02-29T01:02:03:",
        ] {
            assert_eq!(
                date_text(input).unwrap().as_deref(),
                Some("02/29/2024 01:02:03"),
                "{input}"
            );
        }
        assert_eq!(
            date_text("2024-02-29T24:00:00.0000000Z")
                .unwrap()
                .as_deref(),
            Some("03/01/2024 00:00:00")
        );
        for input in [
            "2023-02-29T01:02:03Z",
            "0000-01-01T01:02:03Z",
            "2024-02-29t01:02:03Z",
            "2024-02-29T24:01:00Z",
            "2024-02-29T24:00:00.1Z",
            "2024-02-29T01:02:60Z",
            "2024-02-29T01:02:03.12345678Z",
            "2024-02-29T01:02:03.Z",
            "2024-02-29T01:02:03Z ",
            "2024-02-29",
            "True",
            "not a date",
            "2024-02-29T01:02:03+1",
            "2024-02-29T01:02:03+01:0",
        ] {
            assert_eq!(date_text(input).unwrap(), None, "{input}");
        }
        assert!(date_text("9999-12-31T24:00:00Z").is_err());
    }
    #[test]
    fn explicit_offsets_use_roundtrip_local_clock_and_reject_overflow() {
        let expected = Utc
            .with_ymd_and_hms(2024, 2, 29, 0, 2, 3)
            .unwrap()
            .with_timezone(&Local)
            .format("%m/%d/%Y %H:%M:%S")
            .to_string();
        for input in [
            "2024-02-29T01:02:03+01",
            "2024-02-29T01:02:03+01:",
            "2024-02-29T01:02:03+0100",
            "2024-02-29T01:02:03+01:00",
        ] {
            assert_eq!(date_text(input).unwrap(), Some(expected.clone()), "{input}");
        }
        assert!(date_text("0001-01-01T00:00:00+01").is_err());
    }
}
