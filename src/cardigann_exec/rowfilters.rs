//! Pinned CardigannIndexer.ParseRowFilters / TorznabQuery.MatchQueryStringAND.
use super::{allowed, error, Budget, Context, ExecutionError};
use crate::cardigann::Node;
use regex::Regex;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
pub(super) struct RowFilters {
    first_limit: Option<i32>,
    bypass: Vec<&'static str>,
}
impl RowFilters {
    pub(super) fn compile(root: &Node, rows: &Node) -> Result<Self, ExecutionError> {
        let mut first_limit = None;
        if let Some(filters) = rows.get("filters") {
            let items = filters
                .items()
                .ok_or_else(|| error("$.search.rows.filters", "Row filters must be a sequence"))?;
            if items.len() > 32 {
                return Err(error(
                    "$.search.rows.filters",
                    "Row filter count limit exceeded",
                ));
            }
            for (i, f) in items.iter().enumerate() {
                let path = format!("$.search.rows.filters[{i}]");
                allowed(f, &["name", "args"], &path)?;
                match f.string("name") {
                    Some("andmatch") => {
                        let limit = match f.get("args") {
                            None => -1,
                            Some(n) => n
                                .text()
                                .and_then(|s| s.trim().parse::<i32>().ok())
                                .ok_or_else(|| {
                                    error(
                                        format!("{path}.args"),
                                        "andmatch requires a scalar signed 32-bit character limit",
                                    )
                                })?,
                        };
                        first_limit.get_or_insert(limit);
                    }
                    Some("strdump") => {
                        return Err(error(
                            format!("{path}.name"),
                            "Debug row dumping is disabled",
                        ))
                    }
                    _ => return Err(error(format!("{path}.name"), "Unsupported row filter")),
                }
            }
        }
        let caps = root.get("caps");
        let modes = caps.and_then(|n| n.get("modes"));
        let has = |mode: &str, param: &str| {
            modes
                .and_then(|n| n.get(mode))
                .and_then(Node::items)
                .unwrap_or_default()
                .iter()
                .any(|n| n.text().is_some_and(|s| s.eq_ignore_ascii_case(param)))
        };
        // TvSearchImdbAvailable is overridden by Allowtvsearchimdb, not inferred from modes.
        let tv_imdb = caps.and_then(|n| n.string("allowtvsearchimdb")) == Some("true");
        let imdb = tv_imdb || has("movie-search", "imdbid");
        let mut bypass = Vec::new();
        if imdb {
            bypass.extend(["IMDBID", "DoubanID", "TraktID"]);
        }
        if tv_imdb {
            bypass.extend(["TVMazeID", "TVRageID"]);
        }
        if has("movie-search", "tmdbid") || has("tv-search", "tmdbid") {
            bypass.push("TMDBID");
        }
        if has("tv-search", "tvdbid") {
            bypass.push("TVDBID");
        }
        Ok(Self {
            first_limit,
            bypass,
        })
    }
    pub(super) fn prepare(&self, ctx: &Context) -> Result<Option<Matcher>, ExecutionError> {
        let Some(limit) = self.first_limit else {
            return Ok(None);
        };
        if self.bypass.iter().any(|name| {
            ctx.values
                .get(&format!(".Query.{name}"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| !s.is_empty())
        }) {
            return Ok(None);
        }
        let value = |key| {
            ctx.values
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
        };
        let transformed = value(".Keywords");
        let fallback;
        let query = if transformed.trim().is_empty() {
            fallback = fallback_query(
                value(".Query.Q"),
                value(".Query.Season"),
                value(".Query.Ep"),
            )?;
            fallback.as_str()
        } else {
            transformed
        };
        if query.len() > 1024 * 1024 {
            return Err(error(
                "$.search.rows.filters",
                "andmatch query byte limit exceeded",
            ));
        }
        // C# Substring counts UTF16; split surrogate is a non-word separator.
        let mut used = 0;
        let query: String = query
            .chars()
            .take_while(|c| {
                used += c.len_utf16();
                limit <= 0 || used <= limit as usize
            })
            .collect();
        // .NET regex word categories are L/Mn/Nd/Pc, operating on BMP code units.
        let split =
            Regex::new(r"[^[[\p{L}\p{Mn}\p{Nd}\p{Pc}]&&[\u0000-\uFFFF]]]+").expect("fixed regex");
        validate_collation(&query)?;
        let tokens: Vec<_> = split
            .split(&query)
            .filter(|s| {
                s.encode_utf16().count() > 1
                    && !["and", "the", "an"]
                        .iter()
                        .any(|w| s.eq_ignore_ascii_case(w))
            })
            .map(fold)
            .collect();
        if tokens.len() > 4096 {
            return Err(error(
                "$.search.rows.filters",
                "andmatch token limit exceeded",
            ));
        }
        // TorznabQuery caches parts: first andmatch limit wins for every later filter.
        Ok(Some(Matcher { tokens }))
    }
}
fn fallback_query(q: &str, season: &str, ep: &str) -> Result<String, ExecutionError> {
    if q.len() + season.len() + ep.len() > 1024 * 1024 {
        return Err(error(
            "$.search.rows.filters",
            "andmatch query byte limit exceeded",
        ));
    }
    let dashes = Regex::new(r"\p{Pd}+").expect("fixed regex");
    let q = dashes.replace_all(q, "-");
    let chars = Regex::new(r"[\p{L}\p{Nd}\s\-. _()@/'\[\]+%]").expect("fixed regex");
    let mut sanitized = String::new();
    for c in q.chars() {
        let c = match c {
            '`' | '´' | '‘' | '’' => '\'',
            _ => c,
        };
        if (c as u32) <= 0xffff && chars.is_match(c.encode_utf8(&mut [0; 4])) {
            sanitized.push(c);
        }
    }
    let mut episode = String::new();
    if let Ok(season) = season.parse::<i32>() {
        if season < 0 {
            return Err(error(
                "$.search.rows.filters",
                "andmatch negative fallback season unsupported",
            ));
        }
        if season != 0 {
            if let Ok(date) =
                chrono::NaiveDate::parse_from_str(&format!("{season} {ep}"), "%Y %m/%d")
            {
                episode = date.format("%Y.%m.%d").to_string();
            } else if ep.trim().is_empty() {
                episode = format!("S{season:02}");
            } else {
                let normalized: String = ep.chars().filter(|c| c.is_ascii_digit()).collect();
                if ep.contains([',', '.'])
                    || ep.chars().any(|c| c.is_numeric() && !c.is_ascii_digit())
                {
                    return Err(error(
                        "$.search.rows.filters",
                        "andmatch fallback episode numeric coercion unsupported",
                    ));
                }
                let n = if normalized.is_empty() {
                    0
                } else {
                    normalized.parse::<i32>().map_err(|_| {
                        error(
                            "$.search.rows.filters",
                            "andmatch fallback episode integer overflow",
                        )
                    })?
                };
                episode = format!("S{season:02}E{n:02}");
            }
        }
    }
    Ok(format!("{sanitized} {episode}").trim().to_owned())
}
fn validate_collation(value: &str) -> Result<(), ExecutionError> {
    // Keep known linguistic expansion/ignorable cases explicit, rather than approximate.
    static UNSUPPORTED: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"[\p{Cf}ßẞÆæŒœĲĳſØøŁłĐđĦħÐðﬀ-ﬆ]").expect("fixed regex")
    });
    if UNSUPPORTED.is_match(value) {
        return Err(error(
            "$.search.rows.filters",
            "andmatch linguistic collation expansion/ignorable character unsupported",
        ));
    }
    Ok(())
}
fn fold(value: &str) -> String {
    value
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ς' { 'σ' } else { c })
        .collect()
}
pub(super) struct Matcher {
    tokens: Vec<String>,
}
impl Matcher {
    pub(super) fn matches(&self, title: &str, budget: &mut Budget) -> Result<bool, ExecutionError> {
        if self.tokens.is_empty() {
            return Ok(true);
        }
        budget.consume(title.len())?;
        validate_collation(title)?;
        let title = fold(title);
        for token in &self.tokens {
            budget.operation()?;
            budget.consume(title.len())?;
            if !title.contains(token) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_episode_normalizes_numbers_like_parseutil() {
        assert_eq!(fallback_query("a&b", "1", "01A").unwrap(), "ab S01E01");
        assert_eq!(fallback_query("", "1", "Special").unwrap(), "S01E00");
        assert_eq!(fallback_query("", "1", "-2").unwrap(), "S01E02");
        assert_eq!(fallback_query("", "2024", "02/29").unwrap(), "2024.02.29");
        assert!(fallback_query("", "1", "1,000").is_err());
        assert!(fallback_query("", "-1", "1").is_err());
    }
}
