//! Cardigann filters, bounded independently of response admission limits.
use super::dotnet_regex;
use super::request::Charset;
use super::template::{Context, Template, MAX_OUTPUT};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use regex::{Regex, RegexBuilder};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug)]
pub struct Filter {
    name: String,
    args: Vec<String>,
    regex: Option<DefinitionRegex>,
    replacement: Option<Template>,
    date_layout: Option<super::dates::Layout>,
}
const MAX_REGEX_SOURCE: usize = 4096;
const MAX_TRANSLATED_REGEX: usize = 2 * MAX_REGEX_SOURCE;
const REGEX_SIZE_LIMIT: usize = 1024 * 1024;
/// Backtracking steps per search (one `captures` call, all start positions). fancy-regex
/// fails the search with an error when it is exhausted; it never truncates the result.
pub(crate) const BACKTRACK_LIMIT: usize = 100_000;
/// Input cap for backtracking regexes. Look-around delegates rescan text from each attempted
/// position, so a search is quadratic in the worst case even under the backtrack limit.
pub(crate) const MAX_BACKTRACKING_INPUT: usize = 64 * 1024;

/// A compiled definition regex. Patterns Rust's linear-time engine accepts run there; those
/// needing look-around, atomic groups or backreferences run on fancy-regex with the same
/// normalized syntax (docs/dotnet-regex-lookaround.md). Matching can fail only on the
/// backtracking engine, so every match API returns a `Result`.
#[derive(Clone, Debug)]
pub(crate) enum DefinitionRegex {
    Linear(Regex),
    Backtracking(fancy_regex::Regex),
}
/// Capture spans of one match, as byte ranges into the searched text.
pub(crate) struct Captures<'h> {
    text: &'h str,
    spans: Vec<Option<(usize, usize)>>,
}
impl<'h> Captures<'h> {
    pub(crate) fn get(&self, i: usize) -> Option<&'h str> {
        let (start, end) = (*self.spans.get(i)?)?;
        Some(&self.text[start..end])
    }
    fn span(&self, i: usize) -> Option<(usize, usize)> {
        self.spans.get(i).copied().flatten()
    }
}
fn linear_captures<'h>(text: &'h str, caps: &regex::Captures<'h>) -> Captures<'h> {
    let spans = caps.iter().map(|m| m.map(|m| (m.start(), m.end())));
    Captures {
        text,
        spans: spans.collect(),
    }
}
fn backtracking_captures<'h>(
    text: &'h str,
    caps: fancy_regex::Result<fancy_regex::Captures<'h, str>>,
) -> Result<Captures<'h>, String> {
    let caps = caps.map_err(runtime_error)?;
    let spans = caps.iter().map(|m| m.map(|m| (m.start(), m.end())));
    Ok(Captures {
        text,
        spans: spans.collect(),
    })
}
fn runtime_error(e: fancy_regex::Error) -> String {
    match e {
        fancy_regex::Error::RuntimeError(fancy_regex::RuntimeError::BacktrackLimitExceeded) => {
            "regex backtrack limit exceeded".into()
        }
        e => format!("regex runtime error: {e}"),
    }
}
impl DefinitionRegex {
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Linear(re) => re.as_str(),
            Self::Backtracking(re) => re.as_str(),
        }
    }
    pub(crate) fn is_backtracking(&self) -> bool {
        matches!(self, Self::Backtracking(_))
    }
    pub(crate) fn captures_len(&self) -> usize {
        match self {
            Self::Linear(re) => re.captures_len(),
            Self::Backtracking(re) => re.captures_len(),
        }
    }
    fn capture_index(&self, name: &str) -> Option<usize> {
        match self {
            Self::Linear(re) => re.capture_names().position(|n| n == Some(name)),
            Self::Backtracking(re) => re.capture_names().position(|n| n == Some(name)),
        }
    }
    fn check_input(&self, text: &str) -> Result<(), String> {
        if self.is_backtracking() && text.len() > MAX_BACKTRACKING_INPUT {
            return Err("backtracking regex input limit exceeded".into());
        }
        Ok(())
    }
    /// The leftmost-first match, as `Regex.Match` finds it.
    pub(crate) fn captures<'h>(&self, text: &'h str) -> Result<Option<Captures<'h>>, String> {
        self.check_input(text)?;
        match self {
            Self::Linear(re) => Ok(re.captures(text).map(|c| linear_captures(text, &c))),
            Self::Backtracking(re) => re
                .captures(text)
                .transpose()
                .map(|c| backtracking_captures(text, c))
                .transpose(),
        }
    }
    /// Successive non-overlapping matches. Each backtracking search has its own budget;
    /// callers bound the number of matches.
    pub(crate) fn captures_iter<'h>(
        &'h self,
        text: &'h str,
    ) -> Result<Box<dyn Iterator<Item = Result<Captures<'h>, String>> + 'h>, String> {
        self.check_input(text)?;
        Ok(match self {
            Self::Linear(re) => Box::new(
                re.captures_iter(text)
                    .map(move |c| Ok(linear_captures(text, &c))),
            ),
            Self::Backtracking(re) => Box::new(
                re.captures_iter(text)
                    .map(move |c| backtracking_captures(text, c)),
            ),
        })
    }
    #[cfg(test)]
    pub(crate) fn is_match(&self, text: &str) -> bool {
        self.captures(text).unwrap().is_some()
    }
}

/// Compile a definition-sourced (.NET) regex. Jackett constructs these with no options
/// (CardigannIndexer.cs:345/1143/1151), so they are case-sensitive unless inline flags say
/// otherwise; inline options are rejected next to block properties instead of modelling .NET
/// culture case equivalences. Literal syntax is normalized by [`dotnet_regex::normalize`];
/// non-BMP pattern characters are rejected and non-ASCII case folding parity is not claimed.
pub(crate) fn definition_regex(pattern: &str) -> Result<DefinitionRegex, String> {
    if pattern.len() > MAX_REGEX_SOURCE {
        return Err("regex source limit exceeded".into());
    }
    let normalized = dotnet_regex::scan(pattern)?;
    if normalized.text.len() > MAX_TRANSLATED_REGEX {
        return Err("regex translation limit exceeded".into());
    }
    if normalized.backtracking {
        build_backtracking_regex(&normalized.text).map(DefinitionRegex::Backtracking)
    } else {
        build_regex(&normalized.text).map(DefinitionRegex::Linear)
    }
}
pub(crate) fn bounded_regex(pattern: &str) -> Result<Regex, String> {
    if pattern.len() > MAX_REGEX_SOURCE {
        return Err("regex source limit exceeded".into());
    }
    build_regex(pattern)
}
fn build_regex(pattern: &str) -> Result<Regex, String> {
    // Rust's linear-time engine intentionally rejects .NET backreferences,
    // look-around, balancing groups, and conditionals instead of approximating them.
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .nest_limit(64)
        .build()
        .map_err(|e| format!("unsupported/invalid .NET regex: {e}"))
}
fn build_backtracking_regex(pattern: &str) -> Result<fancy_regex::Regex, String> {
    // fancy-regex parses the whole pattern (group nesting is capped at 64) and delegates
    // look-around-free pieces to `regex` with these limits. Its variable-length look-behind
    // feature is disabled: that path stops collecting candidates at the backtrack limit
    // instead of failing, so only constant-width look-behind compiles.
    fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(BACKTRACK_LIMIT)
        .delegate_size_limit(REGEX_SIZE_LIMIT)
        .delegate_dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|e| format!("unsupported/invalid .NET regex: {e}"))
}
pub(crate) fn replace_regex(
    re: &DefinitionRegex,
    input: &str,
    replacement: &str,
) -> Result<String, String> {
    if input.len() > MAX_OUTPUT || replacement.len() > MAX_OUTPUT {
        return Err("regex input limit exceeded".into());
    }
    if replacement.contains("$`")
        || replacement.contains("$'")
        || replacement.contains("$+")
        || replacement.contains("$_")
    {
        return Err("unsupported .NET substitution token".into());
    }
    let mut out = String::new();
    let mut last = 0;
    let mut count = 0;
    for caps in re.captures_iter(input)? {
        let caps = caps?;
        count += 1;
        if count > 8192 {
            return Err("regex match limit exceeded".into());
        }
        let (start, end) = caps.span(0).unwrap();
        out.push_str(&input[last..start]);
        let expanded = expand_replacement(re, &caps, replacement)?;
        if out.len().saturating_add(expanded.len()) > MAX_OUTPUT {
            return Err("regex output limit exceeded".into());
        }
        out.push_str(&expanded);
        last = end;
    }
    if out.len().saturating_add(input.len() - last) > MAX_OUTPUT {
        return Err("regex output limit exceeded".into());
    }
    out.push_str(&input[last..]);
    Ok(out)
}
fn expand_replacement(
    re: &DefinitionRegex,
    caps: &Captures<'_>,
    replacement: &str,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = replacement;
    while let Some(start) = rest.find('$') {
        append_bounded(&mut out, &rest[..start])?;
        rest = &rest[start..];
        if rest.starts_with("$$") {
            append_bounded(&mut out, "$")?;
            rest = &rest[2..];
            continue;
        }
        if rest.starts_with("$&") {
            append_bounded(&mut out, caps.get(0).unwrap())?;
            rest = &rest[2..];
            continue;
        }
        let (name, consumed) = if let Some(tail) = rest.strip_prefix("${") {
            if let Some(end) = tail.find('}') {
                (Some(&tail[..end]), end + 3)
            } else {
                (None, 1)
            }
        } else {
            let len = rest[1..].bytes().take_while(u8::is_ascii_digit).count();
            if len > 0 {
                (Some(&rest[1..1 + len]), len + 1)
            } else {
                (None, 1)
            }
        };
        if let Some(name) = name {
            let index = name
                .parse::<usize>()
                .ok()
                .filter(|i| *i < re.captures_len());
            let named = re.capture_index(name);
            if index.is_some() || named.is_some() {
                if let Some(value) = index.or(named).and_then(|i| caps.get(i)) {
                    append_bounded(&mut out, value)?;
                }
            } else {
                append_bounded(&mut out, &rest[..consumed])?;
            }
        } else {
            append_bounded(&mut out, "$")?;
        }
        rest = &rest[consumed..];
    }
    append_bounded(&mut out, rest)?;
    Ok(out)
}
fn append_bounded(out: &mut String, value: &str) -> Result<(), String> {
    if out.len().saturating_add(value.len()) > MAX_OUTPUT {
        return Err("regex substitution output limit exceeded".into());
    }
    out.push_str(value);
    Ok(())
}

impl Filter {
    pub fn compile(name: &str, args: &[String]) -> Result<Self, String> {
        let (min, max) = match name {
            "querystring" | "timeparse" | "dateparse" | "regexp" | "prepend" | "append"
            | "diacritics" | "validate" => (1, 1),
            "re_replace" | "replace" | "split" | "jsonjoinarray" => (2, 2),
            "trim" | "strdump" => (0, 1),
            "tolower" | "toupper" | "base64decode" | "base64encode" | "urldecode" | "urlencode"
            | "htmldecode" | "htmlencode" | "timeago" | "reltime" | "fuzzytime"
            | "validfilename" | "hexdump" => (0, 0),
            _ => return Err(format!("unsupported filter {name}")),
        };
        if args.len() < min || args.len() > max {
            return Err(format!("{name} expects {min}..={max} arguments"));
        }
        if args.iter().any(|a| a.len() > 65536) {
            return Err("filter argument limit exceeded".into());
        }
        if name == "diacritics" && args[0] != "replace" {
            return Err("unsupported diacritics operation".into());
        }
        if name == "split" {
            if args[0].is_empty() {
                return Err("empty split separator".into());
            }
            args[1].parse::<i64>().map_err(|_| "invalid split index")?;
        }
        if name == "trim" && args.first().is_some_and(String::is_empty) {
            return Err("empty trim character".into());
        }
        if name == "replace" && args[0].is_empty() {
            return Err("empty replacement pattern".into());
        }
        if name == "jsonjoinarray" {
            validate_json_path(&args[0])?;
        }
        let regex = if name == "regexp" || name == "re_replace" {
            Some(definition_regex(&args[0])?)
        } else {
            None
        };
        let replacement = match name {
            "re_replace" | "replace" => Some(Template::compile(&args[1])?),
            "prepend" | "append" => Some(Template::compile(&args[0])?),
            _ => None,
        };
        let date_layout = if name == "dateparse" || name == "timeparse" {
            Some(super::dates::Layout::compile(&args[0])?)
        } else {
            None
        };
        Ok(Self {
            name: name.into(),
            args: args.to_vec(),
            regex,
            replacement,
            date_layout,
        })
    }
    /// Compile for a definition encoding. WebUtilityHelpers.UrlEncode/UrlDecode use the
    /// definition encoding; non-UTF-8 URL codec semantics are not modeled.
    pub fn compile_for(name: &str, args: &[String], charset: Charset) -> Result<Self, String> {
        let filter = Self::compile(name, args)?;
        if charset != Charset::Utf8 && matches!(name, "urlencode" | "urldecode") {
            return Err(format!(
                "{name} with non-UTF-8 definition encoding is unsupported"
            ));
        }
        Ok(filter)
    }
    pub fn apply(&self, input: &str, ctx: &Context) -> Result<String, String> {
        if input.len() > MAX_OUTPUT {
            return Err("filter input limit exceeded".into());
        }
        let arg = || self.args[0].as_str();
        let replacement = || {
            self.replacement
                .as_ref()
                .ok_or("missing compiled replacement")?
                .render(ctx)
        };
        let value = match self.name.as_str() {
            "querystring" => {
                let query = input
                    .split_once('?')
                    .map_or(input, |(_, q)| q)
                    .split('#')
                    .next()
                    .unwrap_or("");
                url::form_urlencoded::parse(query.as_bytes())
                    .find(|(k, _)| k == arg())
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default()
            }
            "regexp" => self
                .regex
                .as_ref()
                .unwrap()
                .captures(input)?
                .and_then(|c| c.get(1))
                .map(String::from)
                .unwrap_or_default(),
            "re_replace" => replace_regex(self.regex.as_ref().unwrap(), input, &replacement()?)?,
            "split" => {
                let values = input
                    .split(arg().chars().next().unwrap())
                    .collect::<Vec<_>>();
                let index = self.args[1].parse::<i64>().unwrap();
                let i = if index < 0 {
                    (values.len() as i64).checked_add(index)
                } else {
                    Some(index)
                }
                .and_then(|i| usize::try_from(i).ok())
                .ok_or("split index out of range")?;
                values.get(i).ok_or("split index out of range")?.to_string()
            }
            "replace" => {
                let rep = replacement()?;
                let matches = input.matches(arg()).count();
                let len = input
                    .len()
                    .saturating_sub(matches.saturating_mul(arg().len()))
                    .saturating_add(matches.saturating_mul(rep.len()));
                if len > MAX_OUTPUT {
                    return Err("replace output limit exceeded".into());
                }
                input.replace(arg(), &rep)
            }
            "trim" => {
                if self.args.is_empty() {
                    input.trim().into()
                } else {
                    input.trim_matches(arg().chars().next().unwrap()).into()
                }
            }
            "prepend" => {
                let mut s = replacement()?;
                if s.len().saturating_add(input.len()) > MAX_OUTPUT {
                    return Err("prepend output limit exceeded".into());
                }
                s.push_str(input);
                s
            }
            "append" => {
                let rep = replacement()?;
                if input.len().saturating_add(rep.len()) > MAX_OUTPUT {
                    return Err("append output limit exceeded".into());
                }
                format!("{input}{rep}")
            }
            "tolower" => input.to_lowercase(),
            "toupper" => input.to_uppercase(),
            "base64encode" => URL_SAFE_NO_PAD.encode(input),
            "base64decode" => String::from_utf8(
                URL_SAFE_NO_PAD
                    .decode(input.trim_end_matches('='))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
            "urlencode" => url_encode(input),
            "urldecode" => {
                url::form_urlencoded::parse(format!("v={}", input.replace('&', "%26")).as_bytes())
                    .next()
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default()
            }
            "htmldecode" => html_escape::decode_html_entities(input).into_owned(),
            "htmlencode" => html_encode(input)?,
            "timeago" | "reltime" => relative_date(input, ctx)?
                .format("%a, %d %b %Y %H:%M:%S +0")
                .to_string(),
            "dateparse" | "timeparse" => {
                match self.date_layout.as_ref().unwrap().parse(input, ctx.now) {
                    Some(d) => d.format("%a, %d %b %Y %H:%M:%S +0").to_string(),
                    None => input.into(),
                }
            }
            "fuzzytime" => parse_fuzzy_date(input, ctx)?
                .format("%a, %d %b %Y %H:%M:%S +0")
                .to_string(),
            // Upstream Path.GetInvalidFileNameChars on the supported Linux target.
            "validfilename" => {
                let value = input.replace(['/', '\0'], "_");
                if value.is_empty() {
                    "_".into()
                } else {
                    value
                }
            }
            "diacritics" => {
                static MARKS: std::sync::LazyLock<Regex> =
                    std::sync::LazyLock::new(|| bounded_regex(r"\p{Mn}").expect("fixed regex"));
                let decomposed = input.nfd().collect::<String>();
                MARKS.replace_all(&decomposed, "").nfc().collect()
            }
            "jsonjoinarray" => {
                let json: serde_json::Value =
                    serde_json::from_str(input).map_err(|e| e.to_string())?;
                let value = json_path(&json, arg())?;
                let array = value.as_array().ok_or("jsonjoinarray path is not array")?;
                if array.len() > 4096 {
                    return Err("jsonjoinarray item limit exceeded".into());
                }
                let mut output = String::new();
                for (i, v) in array.iter().enumerate() {
                    if i > 0 {
                        output.push_str(&self.args[1]);
                    }
                    match v {
                        serde_json::Value::String(s) => output.push_str(s),
                        _ => output.push_str(&v.to_string()),
                    }
                    if output.len() > MAX_OUTPUT {
                        return Err("jsonjoinarray output limit exceeded".into());
                    }
                }
                output
            }
            "validate" => {
                let split = |c: char| ", /)(.;[]\"|:".contains(c);
                let lower = input.to_lowercase();
                let allowed = self.args[0].to_lowercase();
                let values = lower
                    .split(split)
                    .filter(|s| !s.is_empty())
                    .collect::<std::collections::HashSet<_>>();
                let mut seen = std::collections::HashSet::new();
                allowed
                    .split(split)
                    .filter(|s| !s.is_empty() && values.contains(s) && seen.insert(*s))
                    .collect::<Vec<_>>()
                    .join(",")
            }
            "hexdump" | "strdump" => input.into(),
            _ => return Err("unsupported compiled filter".into()),
        };
        if value.len() > MAX_OUTPUT {
            return Err("filter output limit exceeded".into());
        }
        Ok(value)
    }
}
fn html_encode(input: &str) -> Result<String, String> {
    let mut output = String::new();
    for c in input.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            c if (160..256).contains(&(c as u32)) || c as u32 > 65535 => {
                output.push_str(&format!("&#{};", c as u32))
            }
            _ => output.push(c),
        }
        if output.len() > MAX_OUTPUT {
            return Err("HTML encode output limit exceeded".into());
        }
    }
    Ok(output)
}

/// UTF-8 WebUtility.UrlEncode safe set, used by the upstream helper.
pub(crate) fn url_encode(input: &str) -> String {
    url_encode_bytes(input.as_bytes())
}
/// WebUtility.UrlEncodeToBytes over already-encoded text bytes.
pub(crate) fn url_encode_bytes(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'*'
            | b'('
            | b')' => out.push(b as char),
            b' ' => out.push('+'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 15) as usize] as char);
            }
        }
    }
    out
}

fn validate_json_path(path: &str) -> Result<(), String> {
    if path.len() > 4096
        || path
            .chars()
            .any(|c| !(c.is_alphanumeric() || "._$[]".contains(c)))
    {
        return Err(
            "unsupported jsonjoinarray JSONPath (only object names and numeric indices)".into(),
        );
    }
    Ok(())
}
fn json_path<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Result<&'a serde_json::Value, String> {
    let mut current = value;
    for part in path
        .trim_start_matches('$')
        .trim_start_matches('.')
        .split('.')
    {
        if part.is_empty() {
            continue;
        }
        let mut pieces = part.split('[');
        let key = pieces.next().unwrap();
        if !key.is_empty() {
            current = current.get(key).ok_or("jsonjoinarray missing path")?;
        }
        for index in pieces {
            let i = index
                .trim_end_matches(']')
                .parse::<usize>()
                .map_err(|_| "invalid JSONPath index")?;
            current = current.get(i).ok_or("jsonjoinarray missing index")?;
        }
    }
    Ok(current)
}
fn parse_date(s: &str, format: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(s, format)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, format)
                .ok()
                .map(|d| d.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(s, format)
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc())
        })
}
fn relative_date(input: &str, ctx: &Context) -> Result<DateTime<Utc>, String> {
    let lower = input.to_lowercase();
    if lower.trim() == "now" {
        return Ok(ctx.now);
    }
    let cleaned = lower.replace(',', "");
    let text = cleaned
        .split_whitespace()
        .filter(|s| *s != "and")
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.strip_suffix(" ago").unwrap_or(&text).trim();
    let re = bounded_regex(r"([0-9]+(?:\.[0-9]+)?)\s*([a-z]+)")?;
    let mut last = 0;
    let mut seconds = 0.0;
    let mut matched = false;
    for c in re.captures_iter(text) {
        let matched_span = c.get(0).unwrap();
        if !text[last..matched_span.start()].trim().is_empty() {
            return Err("unsupported relative time syntax".into());
        }
        last = matched_span.end();
        matched = true;
        let n = c[1]
            .parse::<f64>()
            .map_err(|_| "invalid relative time amount")?;
        let unit = &c[2];
        let factor = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "d" | "day" | "days" => 86400.0,
            "w" | "wk" | "wks" | "week" | "weeks" => 604800.0,
            "mo" | "month" | "months" => 2592000.0,
            "y" | "year" | "years" => 31536000.0,
            _ => return Err("unknown relative time unit".into()),
        };
        seconds += n * factor;
    }
    if !matched || !text[last..].trim().is_empty() {
        return Err("unsupported relative time syntax".into());
    }
    if !seconds.is_finite() || seconds > 315360000000.0 {
        return Err("relative time range exceeded".into());
    }
    ctx.now
        .checked_sub_signed(Duration::milliseconds((seconds * 1000.0) as i64))
        .ok_or("relative time out of range".into())
}
/// Supported deterministic subset of DateTimeUtil.FromUnknown. Unrecognized
/// fuzzy grammars fail explicitly rather than guessing a locale or wall clock.
pub fn parse_fuzzy_date(input: &str, ctx: &Context) -> Result<DateTime<Utc>, String> {
    let text = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
        return text
            .parse::<i64>()
            .ok()
            .and_then(|v| DateTime::from_timestamp(v, 0))
            .ok_or("invalid Unix date".into());
    }
    if text.eq_ignore_ascii_case("now") {
        return Ok(ctx.now);
    }
    // Pinned DateTimeUtil.FromUnknown handles Today/Yesterday before fuzzy parsing.
    // Restrict its locale-dependent DateTime.Parse clock to an invariant 24-hour
    // subset, using the interpreter's injected UTC day (as other relative dates do).
    let lower = text.to_ascii_lowercase();
    for (label, days) in [("today", 0), ("yesterday", 1)] {
        if let Some(rest) = lower
            .strip_prefix(label)
            .filter(|rest| rest.is_empty() || rest.starts_with(' ') || rest.starts_with(','))
        {
            let rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
            let clock = rest.strip_prefix("at ").unwrap_or(rest);
            let valid_shape = matches!(clock.len(), 5 | 8)
                && clock.bytes().enumerate().all(|(i, b)| {
                    if i == 2 || i == 5 {
                        b == b':'
                    } else {
                        b.is_ascii_digit()
                    }
                });
            let time = if clock.is_empty() {
                NaiveTime::from_hms_opt(0, 0, 0)
            } else if valid_shape {
                NaiveTime::parse_from_str(
                    clock,
                    if clock.len() == 5 {
                        "%H:%M"
                    } else {
                        "%H:%M:%S"
                    },
                )
                .ok()
            } else {
                None
            }
            .ok_or("unsupported relative calendar time")?;
            // Chrono accepts leap seconds; the pinned DateTime.Parse path does not.
            if clock.ends_with(":60") {
                return Err("unsupported relative calendar time".into());
            }
            let day = ctx
                .now
                .date_naive()
                .checked_sub_days(chrono::Days::new(days))
                .ok_or("relative calendar date overflow")?;
            return Ok(day.and_time(time).and_utc());
        }
    }
    if text.to_lowercase().ends_with(" ago") {
        return relative_date(&text, ctx);
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(&text) {
        return Ok(d.with_timezone(&Utc));
    }
    if let Ok(d) = DateTime::parse_from_rfc2822(&text) {
        return Ok(d.with_timezone(&Utc));
    }
    for format in [
        "%a, %d %b %Y %H:%M:%S %z",
        "%a, %d %b %Y %H:%M:%S +0",
        "%Y-%m-%d %H:%M:%S",
        // Json.NET date tokens become invariant DateTime strings before fields
        // reach FromUnknown, including a date field with no dateparse filter.
        "%m/%d/%Y %H:%M:%S",
        "%Y-%m-%d",
        "%d %b %Y %H:%M:%S",
    ] {
        if let Some(d) = parse_date(&text, format) {
            return Ok(d);
        }
    }
    Err("unsupported fuzzy date syntax (locale guessing is blocked)".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn relative_calendar_days_use_injected_utc_day_and_bounded_clock() {
        let mut c = ctx();
        c.now = "2024-03-01T00:01:00Z".parse().unwrap();
        for (input, expected) in [
            ("Today 23:59", "2024-03-01T23:59:00Z"),
            ("Yesterday, at 10:30:45", "2024-02-29T10:30:45Z"),
            ("TODAY", "2024-03-01T00:00:00Z"),
            ("yesterday 00:00", "2024-02-29T00:00:00Z"),
        ] {
            assert_eq!(
                parse_fuzzy_date(input, &c).unwrap(),
                expected.parse::<DateTime<Utc>>().unwrap()
            );
        }
        for input in [
            "Today 24:00",
            "Today 10:60",
            "Today 10:00:60",
            "Today 1:30",
            "Today 10:30pm",
            "Today 10:30 +0300",
            "Today junk",
            "Today,at 10:30 trailing",
            "Yesterday -01:30",
            "Today2 10:30",
        ] {
            assert!(parse_fuzzy_date(input, &c).is_err(), "{input}");
        }
    }
    #[test]
    fn dotnet_blocks_match_exact_bmp_intervals_and_complements() {
        // Exact endpoints come from the pinned .NET 9 s_propTable, not Unicode scripts.
        for (name, lo, hi) in [
            ("IsCyrillic", 0x0400, 0x04ff),
            ("IsCJKUnifiedIdeographs", 0x4e00, 0x9fff),
        ] {
            for (pattern, invert) in [
                (format!(r"^\p{{{name}}}$"), false),
                (format!(r"^[\p{{{name}}}]$"), false),
                (format!(r"^\P{{{name}}}$"), true),
                (format!(r"^[^\p{{{name}}}]$"), true),
                (format!(r"^[\P{{{name}}}]$"), true),
                (format!(r"^[^\P{{{name}}}]$"), false),
            ] {
                let re = definition_regex(&pattern).unwrap();
                for code in 0..=0xffff {
                    if let Some(c) = char::from_u32(code) {
                        assert_eq!(
                            re.is_match(&c.to_string()),
                            (lo..=hi).contains(&code) != invert,
                            "{pattern} U+{code:04X}"
                        );
                    }
                }
            }
        }
        // Regression: Rust's Cyrillic script extends beyond the .NET block.
        assert!(bounded_regex(r"^\p{IsCyrillic}$").unwrap().is_match("Ԁ"));
        assert!(!definition_regex(r"^\p{IsCyrillic}$").unwrap().is_match("Ԁ"));
        for c in ["ԯ", "ᲀ", "ⷠ", "Ꙁ", "㐀", "𠀀"] {
            assert!(
                !definition_regex(r"^[\p{IsCyrillic}\p{IsCJKUnifiedIdeographs}]$")
                    .unwrap()
                    .is_match(c)
            );
        }
    }
    #[test]
    fn dotnet_blocks_preserve_captures_filters_and_template_replacements() {
        assert_eq!(apply("regexp", &[r"(\p{IsCyrillic}+)"], "XАБԀY"), "АБ");
        assert_eq!(
            apply(
                "re_replace",
                &[r"(\p{IsCyrillic}+)(\p{IsCJKUnifiedIdeographs}+)", "$2/$1"],
                "АБ中Ԁ"
            ),
            "中/АБԀ"
        );
        let re = definition_regex(r"(?<block>\p{IsCyrillic}+)").unwrap();
        assert_eq!(re.captures_len(), 2);
        assert_eq!(replace_regex(&re, "АԀ", "${block}:$1").unwrap(), "А:АԀ");
        let mut c = ctx();
        c.values
            .insert(".Query.Q".into(), serde_json::json!("АԀ中"));
        assert_eq!(
            Template::compile(r#"{{ re_replace .Query.Q "\p{IsCyrillic}" "X" }}"#)
                .unwrap()
                .render(&c)
                .unwrap(),
            "XԀ中"
        );
        for category in dotnet_regex::DOTNET_CATEGORIES {
            let p = format!(r"\p{{{category}}}");
            assert_eq!(dotnet_regex::normalize(&p).unwrap(), p);
        }
        assert!(definition_regex(r"^\p{L}\p{Mn}$")
            .unwrap()
            .is_match("e\u{301}"));
        assert_eq!(apply("diacritics", &["replace"], "é"), "e");
    }
    #[test]
    fn dotnet_blocks_do_not_reinterpret_escaped_literals_or_uncertain_contexts() {
        let literal = r"\\p\{IsCyrillic\}";
        assert_eq!(dotnet_regex::normalize(literal).unwrap(), literal);
        assert!(definition_regex(literal)
            .unwrap()
            .is_match(r"\p{IsCyrillic}"));
        let mixed = definition_regex(r"^[A\-\p{IsCyrillic}\p{Nd}]$").unwrap();
        for c in ["A", "-", "А", "1"] {
            assert!(mixed.is_match(c));
        }
        assert!(!mixed.is_match("Ԁ"));
        assert!(definition_regex(r"^[]\p{IsCyrillic}]$")
            .unwrap()
            .is_match("]"));
        for pattern in [
            r"\p{iscyrillic}",
            r"\p{Is_Cyrillic}",
            r"\p{Cyrillic}",
            r"\p{IsGreek}",
            r"\pL",
            r"\p{IsCyrillic",
            r"\p{}",
            r"(?i)\p{IsCyrillic}",
            r"(?-i:\p{IsCyrillic})",
            r"(?x)\p{IsCyrillic}",
            r"(?#x)\p{IsCyrillic}",
            r"[a-\p{IsCyrillic}]",
            r"[\p{IsCyrillic}-z]",
            r"[\p{IsCyrillic}-]",
            r"[a&&\p{IsCyrillic}]",
            r"[a~~b]\p{IsCyrillic}",
            r"[a--b]\p{IsCyrillic}",
            r"[a-[b]]\p{IsCyrillic}",
            r"(?=a)\p{IsCyrillic}",
            r"(a)\1\p{IsCyrillic}",
        ] {
            assert!(definition_regex(pattern).is_err(), "{pattern}");
        }
        // The existing ordinary option path remains supported.
        assert!(definition_regex("(?i)abc").unwrap().is_match("ABC"));
    }
    #[test]
    fn dotnet_blocks_keep_source_translation_and_nesting_bounds() {
        assert!(definition_regex(&"a".repeat(4097))
            .unwrap_err()
            .contains("source limit"));
        assert!(definition_regex(&format!(
            "{}{}",
            "(".repeat(100),
            r"\p{IsCyrillic}".to_owned() + &")".repeat(100)
        ))
        .is_err());
        assert!(definition_regex(&format!("{}{}", "[".repeat(4000), r"\p{IsCyrillic}")).is_err());
        let largest = r"\p{IsCyrillic}".repeat(MAX_REGEX_SOURCE / r"\p{IsCyrillic}".len());
        assert!(largest.len() <= MAX_REGEX_SOURCE);
        let expanded = dotnet_regex::normalize(&largest).unwrap();
        assert!(expanded.len() > MAX_REGEX_SOURCE && expanded.len() <= MAX_TRANSLATED_REGEX);
        assert!(definition_regex(&largest).is_ok());
    }
    fn ctx() -> Context {
        Context {
            values: BTreeMap::new(),
            now: DateTime::from_timestamp(1700000000, 0).unwrap(),
        }
    }
    fn apply(name: &str, args: &[&str], s: &str) -> String {
        Filter::compile(
            name,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
        .apply(s, &ctx())
        .unwrap()
    }
    #[test]
    fn upstream_filters() {
        for (name, args, input, expected) in [
            ("split", vec!["::", "-1"], "a::b", "b"),
            ("trim", vec!["xy"], "xxayx", "ay"),
            ("regexp", vec!["a(.*)"], "abc", "bc"),
            ("re_replace", vec!["(a)", "$1$1"], "abc", "aabc"),
            ("replace", vec!["b", "z"], "abc", "azc"),
            ("prepend", vec!["x"], "a", "xa"),
            ("append", vec!["x"], "a", "ax"),
            ("tolower", vec![], "A", "a"),
            ("toupper", vec![], "a", "A"),
            ("base64encode", vec![], "abc?", "YWJjPw"),
            ("base64decode", vec![], "YWJjPw", "abc?"),
            ("urlencode", vec![], "a b", "a+b"),
            ("urldecode", vec![], "a+b%26", "a b&"),
            ("htmldecode", vec![], "&amp;", "&"),
            ("htmlencode", vec![], "<&", "&lt;&amp;"),
            ("diacritics", vec!["replace"], "é", "e"),
            ("validate", vec!["a,b,a"], "B A", "a,b"),
            ("querystring", vec!["q"], "https://x/?q=a+b", "a b"),
            (
                "jsonjoinarray",
                vec!["x", ","],
                "{\"x\":[\"a\",\"b\"]}",
                "a,b",
            ),
            ("validfilename", vec![], "a/b", "a_b"),
            ("hexdump", vec![], "a", "a"),
            ("strdump", vec![], "a", "a"),
        ] {
            assert_eq!(apply(name, &args, input), expected, "{name}");
        }
    }
    #[test]
    fn clocks_and_dates() {
        assert_eq!(parse_fuzzy_date("now", &ctx()).unwrap(), ctx().now);
        assert_eq!(
            parse_fuzzy_date("2 hours ago", &ctx()).unwrap(),
            ctx().now - Duration::hours(2)
        );
        assert_eq!(
            apply("dateparse", &["2006-01-02"], "2024-05-10"),
            "Fri, 10 May 2024 00:00:00 +0"
        );
        assert_eq!(apply("timeparse", &["2006-01-02"], "invalid"), "invalid");
        assert_eq!(apply("reltime", &[], "1d"), "Mon, 13 Nov 2023 22:13:20 +0");
        assert_eq!(apply("timeago", &[], "now"), "Tue, 14 Nov 2023 22:13:20 +0");
        assert_eq!(
            apply("fuzzytime", &[], "1700000000"),
            "Tue, 14 Nov 2023 22:13:20 +0"
        );
    }
    #[test]
    fn rejects_partial_relative_dates() {
        for input in [
            "unknown",
            "snow",
            "-2 days ago",
            "+2 days ago",
            "junk 2 days ago",
            "2 days garbage",
            "2 days ago trailing",
            "1.2.3 hours",
            "1day2junk",
        ] {
            assert!(
                Filter::compile("timeago", &[])
                    .unwrap()
                    .apply(input, &ctx())
                    .is_err(),
                "{input}"
            );
            assert!(parse_fuzzy_date(input, &ctx()).is_err(), "{input}");
        }
        assert_eq!(
            apply("timeago", &[], "2 hours and 1 day ago"),
            "Mon, 13 Nov 2023 20:13:20 +0"
        );
        assert_eq!(apply("htmlencode", &[], "'é/😀"), "&#39;&#233;/&#128512;");
        assert_eq!(apply("urldecode", &[], "a&b+c"), "a&b c");
    }
    #[test]
    fn bounded_and_explicit() {
        assert!(Filter::compile("regexp", &["(?(a)b)".into()]).is_err());
        assert!(Filter::compile("unknown", &[]).is_err());
        assert!(Filter::compile("split", &["".into(), "0".into()]).is_err());
        assert!(Filter::compile("re_replace", &["a".into(), "x".repeat(MAX_OUTPUT)]).is_err());
        assert!(Filter::compile("split", &[",".into(), "9".into()])
            .unwrap()
            .apply("a", &ctx())
            .is_err());
    }
    #[test]
    fn regex_substitutions_stay_bounded() {
        let re = definition_regex("(.*)").unwrap();
        assert!(replace_regex(&re, &"a".repeat(MAX_OUTPUT), "$1$1").is_err());
        assert!(replace_regex(&definition_regex("a").unwrap(), &"a".repeat(8193), "b").is_err());
        assert_eq!(
            replace_regex(
                &definition_regex("(?<word>a)").unwrap(),
                "a",
                "$$:${word}:$&:$99"
            )
            .unwrap(),
            "$:a:a:$99"
        );
        assert!(replace_regex(&re, "a", "$`").is_err());
    }
}
