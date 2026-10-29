//! Bounded cookies belonging to one configured source origin.
//!
//! `cookie` parses Set-Cookie names and attributes. This store applies domain, path,
//! Secure and expiration rules, but never expands a source's HTTP authority: every
//! read/update still requires its original scheme, host and port. It is not a
//! shared browser jar and does not perform public-suffix or cross-site navigation.
use anyhow::{anyhow, bail, Result};
use reqwest::header::{HeaderMap, HeaderValue, SET_COOKIE};

const MAX_COOKIES: usize = 64;
const MAX_HEADER: usize = 16 * 1024;
const MAX_COOKIE: usize = 4096;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    expires: Option<i64>,
    /// A pasted Cookie header has no Domain/Path metadata. Its first update adopts
    /// the server's scope instead of leaving an older unscoped credential behind.
    seed: bool,
}
/// Deliberately no Debug implementation: cookie values are credentials.
#[derive(Clone)]
pub(super) struct Cookies {
    origin: url::Origin,
    entries: Vec<Entry>,
}
impl Cookies {
    pub(super) fn empty(base: &url::Url) -> Self {
        Self {
            origin: base.origin(),
            entries: Vec::new(),
        }
    }
    /// Private disk representation, bound to the configured source and seed by
    /// its fingerprint. Never use this in management responses or diagnostics.
    pub(super) fn snapshot(&self, fingerprint: &str) -> Result<Vec<u8>> {
        serde_json::to_vec(&Snapshot {
            version: 1,
            fingerprint: fingerprint.into(),
            entries: self.entries.clone(),
        })
        .map_err(|_| anyhow!("Unable to encode cookie session"))
    }
    pub(super) fn restore(base: &url::Url, fingerprint: &str, bytes: &[u8]) -> Result<Self> {
        let snapshot: Snapshot =
            serde_json::from_slice(bytes).map_err(|_| anyhow!("Invalid saved cookie session"))?;
        if snapshot.version != 1
            || snapshot.fingerprint != fingerprint
            || snapshot.entries.len() > MAX_COOKIES
        {
            bail!("Invalid saved cookie session");
        }
        let host = base.host_str().unwrap_or_default();
        let mut names = std::collections::BTreeSet::new();
        let mut size = 0;
        for e in &snapshot.entries {
            let value = e
                .value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(&e.value);
            size += e.name.len() + e.value.len() + e.domain.len() + e.path.len();
            if size > MAX_HEADER
                || !super::auth::cookie_pair_valid(&e.name, "x")
                || (!value.is_empty() && !super::auth::cookie_pair_valid(&e.name, value))
                || !e.path.starts_with('/')
                || !e.path.is_ascii()
                || e.path.bytes().any(|b| b.is_ascii_control())
                || e.domain.is_empty()
                || (e.domain != host
                    && !(base.domain().is_some() && host.ends_with(&format!(".{}", e.domain))))
                || !names.insert((&e.name, &e.domain, &e.path))
                || (e.seed
                    && (e.path != "/"
                        || e.domain != host
                        || e.expires.is_some()
                        || e.value.is_empty()
                        || e.secure != (base.scheme() == "https")))
                || (!e.seed
                    && ((e.secure && base.scheme() != "https")
                        || (e.name.starts_with("__Secure-") && !e.secure)
                        || (e.name.starts_with("__Host-")
                            && (!e.secure || e.path != "/" || e.domain != host))))
            {
                bail!("Invalid saved cookie session");
            }
        }
        Ok(Self {
            origin: base.origin(),
            entries: snapshot.entries,
        })
    }
    pub fn new(base: &url::Url, supplied: &str) -> Result<Self> {
        let pairs = super::auth::cookie_pairs(supplied)?;
        let entries = pairs
            .into_iter()
            .map(|(name, value)| Entry {
                name: name.into(),
                value: value.into(),
                domain: base.host_str().unwrap_or_default().into(),
                path: "/".into(),
                secure: base.scheme() == "https",
                expires: None,
                seed: true,
            })
            .collect::<Vec<_>>();
        if entries.len() > MAX_COOKIES {
            bail!("Cookie count exceeds session budget");
        }
        Ok(Self {
            origin: base.origin(),
            entries,
        })
    }
    fn check_origin(&self, url: &url::Url) -> Result<()> {
        if url.origin() != self.origin || !url.username().is_empty() || url.password().is_some() {
            bail!("Cookie request must remain on configured origin");
        }
        Ok(())
    }
    pub fn header(&mut self, url: &url::Url, now: i64) -> Result<Option<HeaderValue>> {
        self.check_origin(url)?;
        self.entries.retain(|e| e.expires.is_none_or(|at| at > now));
        let mut selected: Vec<_> = self
            .entries
            .iter()
            .filter(|e| (!e.secure || url.scheme() == "https") && path_matches(url.path(), &e.path))
            .collect();
        // Stable sorting preserves creation order for equally long paths, including replacements.
        selected.sort_by_key(|e| std::cmp::Reverse(e.path.len()));
        if selected.is_empty() {
            return Ok(None);
        }
        let header = selected
            .iter()
            .map(|e| format!("{}={}", e.name, e.value))
            .collect::<Vec<_>>()
            .join("; ");
        if header.len() > MAX_HEADER {
            bail!("Cookie header exceeds session budget");
        }
        let mut header =
            HeaderValue::from_str(&header).map_err(|_| anyhow!("Invalid session cookie header"))?;
        header.set_sensitive(true);
        Ok(Some(header))
    }
    pub fn update(&mut self, url: &url::Url, headers: &HeaderMap, now: i64) -> Result<bool> {
        self.check_origin(url)?;
        let updates: Vec<_> = headers.get_all(SET_COOKIE).iter().collect();
        if updates.len() > MAX_COOKIES
            || updates.iter().map(|h| h.as_bytes().len()).sum::<usize>() > MAX_HEADER
        {
            bail!("Cookie response exceeds session budget");
        }
        // An oversized/malformed response never applies half a batch of updates.
        let mut next = self.clone();
        next.entries.retain(|e| e.expires.is_none_or(|at| at > now));
        for header in updates {
            if header.as_bytes().len() > MAX_COOKIE {
                bail!("Cookie response exceeds session budget");
            }
            let raw = header
                .to_str()
                .map_err(|_| anyhow!("Invalid session cookie response"))?;
            if raw.chars().any(char::is_control) {
                bail!("Invalid session cookie response");
            }
            let Ok(cookie) = cookie::Cookie::parse(raw) else {
                continue;
            };
            let name = cookie.name();
            let value = cookie.value();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(value);
            // Empty values are legal response cookies. The saved header remains strictly validated.
            if !super::auth::cookie_pair_valid(name, "x")
                || (!unquoted.is_empty()
                    && unquoted != "__jackett_expired__"
                    && !super::auth::cookie_pair_valid(name, unquoted))
            {
                continue;
            }
            let host = url.host_str().unwrap_or_default();
            let domain = cookie
                .domain()
                .filter(|d| !d.is_empty())
                .map(|d| d.trim_start_matches('.').to_ascii_lowercase())
                .unwrap_or_else(|| host.into());
            if domain != host && !(url.domain().is_some() && host.ends_with(&format!(".{domain}")))
            {
                continue;
            }
            let path = cookie
                .path()
                .filter(|p| p.starts_with('/'))
                .map(str::to_owned)
                .unwrap_or_else(|| default_path(url.path()).into());
            let secure = cookie.secure().unwrap_or(false);
            if (secure && url.scheme() != "https")
                || (name.starts_with("__Secure-") && (!secure || url.scheme() != "https"))
                || (name.starts_with("__Host-")
                    && (!secure
                        || url.scheme() != "https"
                        || cookie.domain().is_some()
                        || cookie.path() != Some("/")))
            {
                continue;
            }
            // Pinned Jackett accepts a leading sign and uses the last valid i64
            // Max-Age, including +0. Invalid/overflowing attributes do not hide Expires.
            let max_age = raw
                .split(';')
                .skip(1)
                .filter_map(|attribute| {
                    let (key, value) = attribute.split_once('=')?;
                    key.trim()
                        .eq_ignore_ascii_case("max-age")
                        .then(|| value.trim().parse::<i64>().ok())
                        .flatten()
                })
                .last();
            let expires = if let Some(age) = max_age {
                Some(if age <= 0 {
                    now
                } else {
                    now.saturating_add(age)
                })
            } else {
                // RFC6265 ignores weekday/timezone spelling. Pinned Jackett also
                // accepts UTC, GMT+0000 and incorrect weekdays, unlike strict date parsers.
                raw.split(';')
                    .skip(1)
                    .filter_map(|attribute| {
                        let (key, value) = attribute.split_once('=')?;
                        key.trim()
                            .eq_ignore_ascii_case("expires")
                            .then(|| expires_date(value.trim()))
                            .flatten()
                    })
                    .last()
            };
            let position = next
                .entries
                .iter()
                .position(|e| e.name == name && (e.seed || e.path == path && e.domain == domain));
            if expires.is_some_and(|at| at <= now) || value == "__jackett_expired__" {
                if let Some(n) = position {
                    next.entries.remove(n);
                }
                continue;
            }
            let entry = Entry {
                name: name.into(),
                value: value.into(),
                domain,
                path,
                secure,
                expires,
                seed: false,
            };
            if let Some(n) = position {
                next.entries[n] = entry;
            } else {
                next.entries.push(entry);
            }
        }
        if next.entries.len() > MAX_COOKIES
            || next
                .entries
                .iter()
                .map(|e| e.name.len() + e.value.len() + e.domain.len() + e.path.len())
                .sum::<usize>()
                > MAX_HEADER
        {
            bail!("Cookie state exceeds session budget");
        }
        let changed = self.entries != next.entries;
        *self = next;
        Ok(changed)
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u8,
    fingerprint: String,
    entries: Vec<Entry>,
}
fn default_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(n) if n > 0 => &path[..n],
        _ => "/",
    }
}
fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie
        || request
            .strip_prefix(cookie)
            .is_some_and(|tail| cookie.ends_with('/') || tail.starts_with('/'))
}
/// RFC6265 section 5.1.1 token algorithm; no locale, weekday or zone guessing.
fn expires_date(value: &str) -> Option<i64> {
    let (mut day, mut month, mut year, mut time) = (None, None, None, None);
    fn digits(token: &str, min: usize, max: usize) -> Option<u32> {
        let n = token.bytes().take_while(u8::is_ascii_digit).count();
        (min..=max)
            .contains(&n)
            .then(|| token[..n].parse().ok())
            .flatten()
    }
    for token in value
        .split(|c| matches!(c, '\t' | ' '..='/' | ';'..='@' | '['..='`' | '{'..='~'))
        .filter(|s| !s.is_empty())
    {
        if time.is_none() {
            let parts: Vec<_> = token.splitn(3, ':').collect();
            if parts.len() == 3
                && parts[..2]
                    .iter()
                    .all(|s| s.bytes().all(|b| b.is_ascii_digit()))
            {
                if let (Some(h), Some(m), Some(s)) = (
                    digits(parts[0], 1, 2),
                    digits(parts[1], 1, 2),
                    digits(parts[2], 1, 2),
                ) {
                    time = Some((h, m, s));
                    continue;
                }
            }
        }
        if day.is_none() {
            if let Some(n) = digits(token, 1, 2) {
                day = Some(n);
                continue;
            }
        }
        if month.is_none() {
            if let Some(m) = token.get(..3).and_then(|prefix| {
                [
                    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov",
                    "dec",
                ]
                .iter()
                .position(|m| prefix.eq_ignore_ascii_case(m))
            }) {
                month = Some(m as u32 + 1);
                continue;
            }
        }
        if year.is_none() {
            year = digits(token, 2, 4);
        }
    }
    let mut year = year? as i32;
    if year <= 69 {
        year += 2000;
    } else if year <= 99 {
        year += 1900;
    }
    if year < 1601 {
        return None;
    }
    let (h, m, s) = time?;
    Some(
        chrono::NaiveDate::from_ymd_opt(year, month?, day?)?
            .and_hms_opt(h, m, s)?
            .and_utc()
            .timestamp(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: i64 = 1_793_145_600;
    fn base() -> url::Url {
        url::Url::parse("https://tracker.example/account/login").unwrap()
    }
    fn apply(jar: &mut Cookies, target: &url::Url, values: &[&str], now: i64) -> Result<()> {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(SET_COOKIE, HeaderValue::from_str(value).unwrap());
        }
        jar.update(target, &headers, now).map(|_| ())
    }
    fn header(jar: &mut Cookies, target: &url::Url, now: i64) -> String {
        jar.header(target, now)
            .unwrap()
            .map(|h| h.to_str().unwrap().to_owned())
            .unwrap_or_default()
    }
    #[test]
    fn renew_expire_delete_and_last_header_win_without_reseeding() {
        let url = base();
        let mut jar = Cookies::new(&url, "session=old; uid=42").unwrap();
        apply(
            &mut jar,
            &url,
            &[
                "session=first; Path=/",
                "session=new; Path=/; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
            ],
            NOW,
        )
        .unwrap();
        assert_eq!(header(&mut jar, &url, NOW), "session=new; uid=42");
        assert_eq!(header(&mut jar, &url, NOW + 60), "uid=42");
        apply(
            &mut jar,
            &url,
            &["uid=deleted; Path=/; Max-Age=-1"],
            NOW + 61,
        )
        .unwrap();
        assert_eq!(header(&mut jar, &url, NOW + 61), "");
        apply(
            &mut jar,
            &url,
            &["x=present; Path=/", "x=__jackett_expired__; Path=/"],
            NOW,
        )
        .unwrap();
        assert_eq!(header(&mut jar, &url, NOW), "");
    }
    #[test]
    fn default_path_domain_secure_ordering_and_source_isolation() {
        let url = base();
        let mut jar = Cookies::new(&url, "session=root").unwrap();
        apply(&mut jar, &url, &["session=root; Path=/"], NOW).unwrap();
        apply(
            &mut jar,
            &url,
            &[
                "session=scoped",
                "parent=yes; Domain=.example; Path=/; Secure",
                "alien=no; Domain=other.example; Path=/",
                "empty=; Path=/",
                "quoted=\"valid\"; Path=/",
            ],
            NOW,
        )
        .unwrap();
        assert_eq!(
            header(&mut jar, &url, NOW),
            "session=scoped; session=root; parent=yes; empty=; quoted=\"valid\""
        );
        assert_eq!(
            header(&mut jar, &url.join("/accounting").unwrap(), NOW),
            "session=root; parent=yes; empty=; quoted=\"valid\""
        );
        assert!(jar
            .header(&url::Url::parse("http://tracker.example/").unwrap(), NOW)
            .is_err());
        assert!(jar
            .header(
                &url::Url::parse("https://tracker.example:8443/").unwrap(),
                NOW
            )
            .is_err());
        assert!(jar
            .header(&url::Url::parse("https://sibling.example/").unwrap(), NOW)
            .is_err());
        let mut other = Cookies::new(&url, "session=other").unwrap();
        assert_eq!(header(&mut other, &url, NOW), "session=other");
        let http = url::Url::parse("http://tracker.example/").unwrap();
        let mut insecure = Cookies::new(&http, "session=old").unwrap();
        apply(&mut insecure, &http, &["session=new; Secure; Path=/"], NOW).unwrap();
        assert_eq!(header(&mut insecure, &http, NOW), "session=old");
    }
    #[test]
    fn expiration_dates_prefixes_and_empty_response_values() {
        let url = base();
        let mut jar = Cookies::new(&url, "session=old").unwrap();
        apply(
            &mut jar,
            &url,
            &[
                "session=deleted; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
                "__Host-good=yes; Path=/; Secure",
                "__Host-domain=no; Domain=tracker.example; Path=/; Secure",
                "__Secure-bad=no; Path=/",
                "x=; Path=/",
            ],
            NOW,
        )
        .unwrap();
        assert_eq!(header(&mut jar, &url, NOW), "__Host-good=yes; x=");
        apply(
            &mut jar,
            &url,
            &["x=new; Path=/; Max-Age=0; Expires=Tue, 01 Jan 2030 00:00:00 GMT"],
            NOW,
        )
        .unwrap();
        assert_eq!(header(&mut jar, &url, NOW), "__Host-good=yes");
    }
    #[test]
    fn expires_accepts_cookie_date_formats_and_rejects_invalid_components() {
        // RFC6265 token parsing ignores weekday and zone text. The nonstandard
        // zero-offset suffixes also appear in pinned Jackett's CookieUtil cases.
        let expected = chrono::NaiveDate::from_ymd_opt(2021, 6, 9)
            .unwrap()
            .and_hms_opt(10, 18, 14)
            .unwrap()
            .and_utc()
            .timestamp();
        for text in [
            "Wed, 09 Jun 2021 10:18:14 GMT",
            "Monday, 09-Jun-21 10:18:14 UTC",
            "Thu Jun 9 10:18:14 2021",
            "Fri, 09 Jun 2021 10:18:14 GMT+0000",
            "09 jun 2021 10:18:14 GMT+00:00",
            "09 Jun 2021 10:18:14:ignored",
        ] {
            assert_eq!(expires_date(text), Some(expected), "{text}");
        }
        for text in [
            "",
            "garbage",
            "09 Jun 1599 10:18:14",
            "31 Feb 2021 10:18:14",
            "09 Jun 2021 24:00:00",
            "09 Jun 2021 10:60:00",
            "09 Jun 2021 10:18:60",
            "09 Jun 2021",
            "099 Jun 2021 10:18:14",
            "09 Jun 20210 10:18:14",
        ] {
            assert_eq!(expires_date(text), None, "{text}");
        }
        for (year, expected_year) in [("69", 2069), ("70", 1970), ("00", 2000)] {
            let value = expires_date(&format!("01 Jan {year} 00:00:00")).unwrap();
            assert_eq!(
                value,
                chrono::NaiveDate::from_ymd_opt(expected_year, 1, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp()
            );
        }
    }
    #[test]
    fn signed_max_age_and_last_valid_attribute_control_expiration() {
        let url = base();
        for suffix in [
            "Max-Age=+0; Expires=01 Jan 2099 00:00:00 GMT",
            "Max-Age=10; Max-Age=-1",
            "Max-Age=9223372036854775808; Expires=01 Jan 1970 00:00:00 UTC",
            "Max-Age=invalid; Expires=01 Jan 1970 00:00:00 GMT+0000",
        ] {
            let mut jar = Cookies::new(&url, "session=old").unwrap();
            apply(
                &mut jar,
                &url,
                &[&format!("session=new; Path=/; {suffix}")],
                NOW,
            )
            .unwrap();
            assert_eq!(header(&mut jar, &url, NOW), "", "{suffix}");
        }
        let mut jar = Cookies::new(&url, "session=old").unwrap();
        apply(&mut jar, &url, &["session=new; Path=/; Max-Age=+10; Max-Age=invalid; Expires=01 Jan 1970 00:00:00 GMT"], NOW).unwrap();
        assert_eq!(header(&mut jar, &url, NOW + 9), "session=new");
        assert_eq!(header(&mut jar, &url, NOW + 10), "");
    }
    #[test]
    fn first_response_adopts_the_seed_cookie_scope_and_deletion_does_not_revive_it() {
        let url = base();
        let mut jar = Cookies::new(&url, "session=old").unwrap();
        apply(
            &mut jar,
            &url,
            &["session=new; Domain=.example; Path=/account"],
            NOW,
        )
        .unwrap();
        assert_eq!(header(&mut jar, &url, NOW), "session=new");
        assert_eq!(header(&mut jar, &url.join("/outside").unwrap(), NOW), "");
        apply(
            &mut jar,
            &url,
            &["session=deleted; Domain=.example; Path=/account; Max-Age=0"],
            NOW,
        )
        .unwrap();
        assert_eq!(header(&mut jar, &url, NOW), "");
        assert_eq!(header(&mut jar, &url.join("/outside").unwrap(), NOW), "");
    }
    #[test]
    fn bounded_updates_are_atomic_and_errors_never_include_values() {
        let url = base();
        let mut jar = Cookies::new(&url, "session=old").unwrap();
        let huge = format!("session={}", "private-secret".repeat(400));
        let error = apply(&mut jar, &url, &["session=new; Path=/", &huge], NOW)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-secret"));
        assert_eq!(header(&mut jar, &url, NOW), "session=old");
        let values: Vec<_> = (0..65).map(|n| format!("c{n}=value; Path=/")).collect();
        assert!(apply(
            &mut jar,
            &url,
            &values.iter().map(String::as_str).collect::<Vec<_>>(),
            NOW
        )
        .is_err());
        assert_eq!(header(&mut jar, &url, NOW), "session=old");
        let foreign = url::Url::parse("https://elsewhere.example/").unwrap();
        assert!(apply(&mut jar, &foreign, &["session=new"], NOW).is_err());
    }
}
