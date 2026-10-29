//! Bounded invariant-culture subset of pinned DateTimeUtil.ParseDateTimeGoLang.
//! .NET layouts are attempted first; only the Go conversion fallback rolls back
//! a future date whose layout omits the year. No host locale or wall clock is used.
use super::filters::bounded_regex;
use chrono::{
    format::{parse, Parsed, StrftimeItems},
    DateTime, Datelike, Duration, Utc,
};
use regex::Regex;

#[derive(Clone, Debug)]
pub(super) struct Layout {
    direct: Option<Format>,
    fallback: Option<Format>,
    fallback_rollover: bool,
}
#[derive(Clone, Debug)]
struct Format {
    chrono: String,
    shape: Regex,
    year: bool,
    date: bool,
    hour: bool,
}
impl Layout {
    pub(super) fn compile(layout: &str) -> Result<Self, String> {
        if layout.len() > 256 {
            return Err("date layout limit exceeded".into());
        }
        // The pinned helper's initial TryParseExact is conditional on these letters.
        let direct = if layout.to_ascii_lowercase().contains(['y', 'h', 'd']) {
            Format::compile(layout).ok()
        } else {
            None
        };
        let mut converted = layout.to_owned();
        for (from, to) in [
            ("2006", "yyyy"),
            ("06", "yy"),
            ("January", "MMMM"),
            ("Jan", "MMM"),
            ("01", "MM"),
            ("Monday", "dddd"),
            ("Mon", "ddd"),
            ("02", "dd"),
            ("2", "d"),
            ("05", "ss"),
            ("15", "HH"),
            ("03", "hh"),
            ("3", "h"),
            ("04", "mm"),
            ("4", "m"),
            ("5", "s"),
            ("1", "M"),
            (".0000", "ffff"),
            (".000", "fff"),
            (".00", "ff"),
            (".0", "f"),
            (".9999", "FFFF"),
            (".999", "FFF"),
            (".99", "FF"),
            (".9", "F"),
            ("PM", "tt"),
            ("pm", "tt"),
            ("Z07:00", "'Z'zzz"),
            ("Z07", "'Z'zz"),
            ("-07:00", "zzz"),
            ("-07", "zz"),
        ] {
            converted = converted.replace(from, to);
        }
        let fallback = Format::compile(&converted).ok();
        if direct.is_none() && fallback.is_none() {
            return Err("unsupported invariant .NET/Go date layout".into());
        }
        Ok(Self {
            direct,
            fallback,
            fallback_rollover: !converted.contains("yy"),
        })
    }
    pub(super) fn parse(&self, input: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let input = input.trim();
        if let Some(value) = self.direct.as_ref().and_then(|f| f.parse(input, now)) {
            return Some(value);
        }
        let f = self.fallback.as_ref()?;
        let mut value = f.parse(input, now)?;
        if self.fallback_rollover && value > now {
            // AddYears(-1) clamps February 29 to February 28.
            value = value
                .with_year(value.year() - 1)
                .or_else(|| (value - Duration::days(1)).with_year(value.year() - 1))?;
        }
        if !(1..=9999).contains(&value.year()) {
            return None;
        }
        Some(value)
    }
}
impl Format {
    fn compile(layout: &str) -> Result<Self, String> {
        if layout.len() == 1 {
            return Err("standard single-letter date formats unsupported".into());
        }
        let mut format = String::new();
        let mut shape = String::from("(?i)\\A");
        let mut rest = layout;
        let (mut year, mut date, mut hour) = (false, false, false);
        while !rest.is_empty() {
            let c = rest.chars().next().unwrap();
            if c == '\'' || c == '"' {
                rest = &rest[c.len_utf8()..];
                let mut closed = false;
                while let Some(ch) = rest.chars().next() {
                    rest = &rest[ch.len_utf8()..];
                    if ch == c {
                        closed = true;
                        break;
                    }
                    let ch = if ch == '\\' {
                        let v = rest.chars().next().ok_or("unterminated date escape")?;
                        rest = &rest[v.len_utf8()..];
                        v
                    } else {
                        ch
                    };
                    literal(ch, &mut format, &mut shape);
                }
                if !closed {
                    return Err("unterminated date literal".into());
                }
                continue;
            }
            if c == '\\' {
                rest = &rest[1..];
                let ch = rest.chars().next().ok_or("unterminated date escape")?;
                literal(ch, &mut format, &mut shape);
                rest = &rest[ch.len_utf8()..];
                continue;
            }
            if !c.is_ascii_alphabetic() {
                if c == '%' {
                    return Err("percent date specifiers unsupported".into());
                }
                literal(c, &mut format, &mut shape);
                rest = &rest[c.len_utf8()..];
                continue;
            }
            let n = rest.chars().take_while(|v| *v == c).count();
            let token = &rest[..n];
            let (fmt, re) = match token {
                "yyyy" => {
                    year = true;
                    ("%Y", "[0-9]{4}")
                }
                "yy" => {
                    year = true;
                    ("%y", "[0-9]{2}")
                }
                "MMMM" => {
                    date = true;
                    ("%B", "(?:January|February|March|April|May|June|July|August|September|October|November|December)")
                }
                "MMM" => {
                    date = true;
                    ("%b", "(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)")
                }
                "MM" => {
                    date = true;
                    ("%m", "[0-9]{2}")
                }
                "M" => {
                    date = true;
                    ("%m", "[0-9]{1,2}")
                }
                "dddd" => (
                    "%A",
                    "(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)",
                ),
                "ddd" => ("%a", "(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)"),
                "dd" => {
                    date = true;
                    ("%d", "[0-9]{2}")
                }
                "d" => {
                    date = true;
                    ("%d", "[0-9]{1,2}")
                }
                "HH" => {
                    hour = true;
                    ("%H", "[0-9]{2}")
                }
                "H" => {
                    hour = true;
                    ("%H", "[0-9]{1,2}")
                }
                "hh" => {
                    hour = true;
                    ("%I", "[0-9]{2}")
                }
                "h" => {
                    hour = true;
                    ("%I", "[0-9]{1,2}")
                }
                "mm" => ("%M", "[0-9]{2}"),
                "m" => ("%M", "[0-9]{1,2}"),
                "ss" => ("%S", "[0-9]{2}"),
                "s" => ("%S", "[0-9]{1,2}"),
                "tt" => ("%p", "(?:AM|PM)"),
                "zzz" => ("%:z", "[+-][0-9]{2}:[0-9]{2}"),
                "zz" => ("%#z", "[+-][0-9]{2}"),
                "T" => ("T", "T"),
                "Z" => ("Z", "Z"),
                // Calendar/zone/era abbreviations and locale-dependent single-letter
                // standard formats remain explicit blockers.
                _ => return Err("unsupported invariant date token".into()),
            };
            format.push_str(fmt);
            shape.push_str(re);
            rest = &rest[n..];
        }
        if !date && !hour && !year {
            return Err("date layout contains no calendar/time fields".into());
        }
        shape.push_str("\\z");
        Ok(Self {
            chrono: format,
            shape: bounded_regex(&shape)?,
            year,
            date,
            hour,
        })
    }
    fn parse(&self, input: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        if !self.shape.is_match(input) {
            return None;
        }
        let mut p = Parsed::new();
        parse(&mut p, input, StrftimeItems::new(&self.chrono)).ok()?;
        if let Some(y) = p.year_mod_100() {
            if p.year().is_none() {
                // GregorianCalendar.TwoDigitYearMax is pinned here to 2049 (.NET 8/9).
                p.set_year(if y <= 49 { 2000 + y } else { 1900 + y } as i64)
                    .ok()?;
            }
        }
        if p.year().is_none() {
            p.set_year(now.year() as i64).ok()?;
        }
        if p.month().is_none() {
            p.set_month(if self.year { 1 } else { now.month() } as i64)
                .ok()?;
        }
        if p.day().is_none() {
            p.set_day(if self.date || self.year { 1 } else { now.day() } as i64)
                .ok()?;
        }
        if !self.hour {
            p.set_hour(0).ok()?;
        }
        if p.hour_div_12().is_none() && p.hour_mod_12().is_some() {
            p.set_ampm(false).ok()?;
        }
        if p.minute().is_none() {
            p.set_minute(0).ok()?;
        }
        if p.second().is_none() {
            p.set_second(0).ok()?;
        }
        let offset = p.offset().unwrap_or(0);
        // Offsets above fourteen hours and leap seconds are outside DateTime's range.
        if offset.abs() > 14 * 3600 || p.second()? > 59 {
            return None;
        }
        let local = p.to_naive_datetime_with_offset(offset).ok()?;
        if !(1..=9999).contains(&local.year()) {
            return None;
        }
        let utc = local.checked_sub_signed(Duration::seconds(offset as i64))?;
        if !(1..=9999).contains(&utc.year()) {
            return None;
        }
        Some(utc.and_utc())
    }
}
fn literal(c: char, format: &mut String, shape: &mut String) {
    if c == '%' {
        format.push('%');
    }
    format.push(c);
    shape.push_str(&regex::escape(&c.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap()
    }
    fn date(layout: &str, input: &str) -> String {
        Layout::compile(layout)
            .unwrap()
            .parse(input, now())
            .unwrap()
            .to_rfc3339()
    }
    #[test]
    fn pinned_dotnet_calendar_layouts() {
        assert_eq!(
            date("yyyy-MM-dd HH:mm:ss zzz", "2024-02-29 00:30:00 +02:00"),
            "2024-02-28T22:30:00+00:00"
        );
        assert_eq!(
            date("MM/dd/yyyy HH:mm:ss zzz", "03/04/2024 12:30:00 -05:30"),
            "2024-03-04T18:00:00+00:00"
        );
        assert_eq!(
            date("yyyy-MM-ddHH:mm:ss zzz", "2024-02-2912:00:00 +00:00"),
            "2024-02-29T12:00:00+00:00"
        );
        assert_eq!(
            date(
                "ddd, dd MMM yyyy HH:mm:ss zzz",
                "Thu, 29 Feb 2024 12:00:00 +00:00"
            ),
            "2024-02-29T12:00:00+00:00"
        );
        assert_eq!(
            date("dd.MM.yyyy HH:mm", "04.03.2024 13:09"),
            "2024-03-04T13:09:00+00:00"
        );
        assert_eq!(
            date("yyyy-MM-dd'T'HH:mm:ss'Z'", "2024-03-04T13:09:01Z"),
            "2024-03-04T13:09:01+00:00"
        );
    }
    #[test]
    fn native_layout_first_and_go_rollover_differ() {
        // 1337x's ordinal suffix is removed by its preceding regex filter.
        assert_eq!(
            date("htt MMM. d", "7am Sep. 14"),
            "2026-09-14T07:00:00+00:00"
        );
        assert_eq!(date("MMM. d yy", "Apr. 18 11"), "2011-04-18T00:00:00+00:00");
        assert_eq!(date("Jan 2 3PM", "Dec 31 7PM"), "2025-12-31T19:00:00+00:00");
        assert_eq!(date("MMM d", "Dec 31"), "2026-12-31T00:00:00+00:00");
        assert_eq!(
            date("2006-01-02", "2024-02-29"),
            "2024-02-29T00:00:00+00:00"
        );
        assert_eq!(date("HH:mm", "23:59"), "2026-01-02T23:59:00+00:00");
        assert_eq!(date("hh:mm", "12:01"), "2026-01-02T00:01:00+00:00");
        assert_eq!(date("hh:mm", "01:01"), "2026-01-02T01:01:00+00:00");
        assert_eq!(date("MMM d yy", "Jan 1 49"), "2049-01-01T00:00:00+00:00");
        assert_eq!(date("MMM d yy", "Jan 1 50"), "1950-01-01T00:00:00+00:00");
    }
    #[test]
    fn partial_times_year_rollover_and_calendar_bounds() {
        assert_eq!(date("hh:mm", "12:01"), "2026-01-02T00:01:00+00:00");
        assert_eq!(date("hh:mm", "01:01"), "2026-01-02T01:01:00+00:00");
        assert_eq!(date("yyyy", "2024"), "2024-01-01T00:00:00+00:00");
        let clock = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        // Fallback parses February 29 in the injected leap year then AddYears clamps.
        assert_eq!(
            Layout::compile("Jan 2")
                .unwrap()
                .parse("Feb 29", clock)
                .unwrap()
                .to_rfc3339(),
            "2023-02-28T00:00:00+00:00"
        );
        let direct = Layout::compile("MMM d").unwrap();
        assert_eq!(
            direct.parse("Feb 29", clock).unwrap().to_rfc3339(),
            "2024-02-29T00:00:00+00:00"
        );
        for input in [
            "0001-01-01 +01:00",
            "9999-12-31 -14:00",
            "0000-12-31 -14:00",
        ] {
            let layout = Layout::compile("yyyy-MM-dd zzz").unwrap();
            // Midnight at9999-12-31 -14 remains within the calendar.
            if input.starts_with("9999") {
                assert!(layout.parse(input, now()).is_some());
            } else {
                assert!(layout.parse(input, now()).is_none());
            }
        }
        assert!(Layout::compile("yyyy-MM-dd HH:mm zzz")
            .unwrap()
            .parse("9999-12-31 23:59 -01:00", now())
            .is_none());
    }
    #[test]
    fn fallback_checks_converted_literal_year_text_and_lower_calendar_bound() {
        assert_eq!(date("Jan 2 'yy'", "Dec 31 yy"), "2026-12-31T00:00:00+00:00");
        let clock = Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0).unwrap();
        assert!(Layout::compile("Jan 2")
            .unwrap()
            .parse("Dec 31", clock)
            .is_none());
        assert_eq!(
            date("yyyy-MM-dd HH:mm zz", "2024-02-29 01:00 +02"),
            "2024-02-28T23:00:00+00:00"
        );
    }
    #[test]
    fn calendar_ordinals_and_escaped_literals() {
        assert_eq!(
            date("d'st' MMMM yyyy", "1st March 2024"),
            "2024-03-01T00:00:00+00:00"
        );
        assert_eq!(
            date("yyyy\\y MM\\M dd\\d", "2024y 03M 01d"),
            "2024-03-01T00:00:00+00:00"
        );
        assert_eq!(
            date("yyyy-MM-dd 'o\\'clock' HH:mm", "2024-03-01 o'clock 12:00"),
            "2024-03-01T12:00:00+00:00"
        );
    }
    #[test]
    fn invalid_dates_are_not_guessed() {
        for (layout, input) in [
            ("yyyy-MM-dd", "2023-02-29"),
            ("yyyy-MM-dd", "2024-13-01"),
            ("yyyy-MM-dd", "2024-2-01"),
            ("yyyy-MM-dd", "0000-01-01"),
            ("yyyy-MM-dd HH:mm:ss", "2024-01-01 23:59:60"),
            ("yyyy-MM-dd zzz", "2024-01-01 +14:01"),
            ("yyyy-MM-dd zzz", "2024-01-01 +24:00"),
            ("dd MMM yyyy", "01 Mär 2024"),
            ("ddd, dd MMM yyyy", "Fri, 29 Feb 2024"),
            ("htt MMM. d", "0pm Sep. 14"),
        ] {
            assert!(
                Layout::compile(layout)
                    .unwrap()
                    .parse(input, now())
                    .is_none(),
                "{layout}: {input}"
            );
        }
        for layout in [
            "d",
            "MST",
            "yyyy-MM-dd K",
            "yyyy-MM-dd 'unterminated",
            "yyyy-MM-dd %q",
        ] {
            assert!(Layout::compile(layout).is_err(), "{layout}");
        }
    }
}
