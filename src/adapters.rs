//! Native adapters. This deliberately does not interpret Cardigann definitions.
use crate::models::{Indexer, Release, SearchQuery};
use anyhow::{anyhow, bail, Context, Result};
use chrono::DateTime;
use futures_util::StreamExt;
use scraper::{Html, Selector};
use serde_json::{json, Value};
use url::Url;
const MAX_BODY: usize = 8 * 1024 * 1024;

fn http_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| anyhow!("Invalid HTTP URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("URL must use HTTP(S) without embedded credentials");
    }
    Ok(url)
}
fn definition(indexer: &Indexer) -> Result<&Value> {
    indexer
        .definition
        .as_ref()
        .ok_or_else(|| anyhow!("This adapter requires a definition"))
}
fn cfg<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
/// UNIT3D opt-in matching the 0dayfiles-api.yml checkbox of the same name.
const SINGLE_FILE_TITLE: &str = "single_file_release_use_filename";
/// UNIT3D opt-in matching the 0dayfiles-api.yml `keywordsfilters` re_replace `\\.` -> ` `.
const KEYWORDS_REPLACE_DOTS: &str = "keywords_replace_dots";
fn selector(raw: &str) -> Result<Selector> {
    Selector::parse(raw).map_err(|_| anyhow!("Invalid CSS selector in adapter definition"))
}
pub fn validate(indexer: &Indexer) -> Result<()> {
    http_url(&indexer.url)?;
    if indexer.kind == "cardigann" {
        crate::builtin::Runtime::compile(indexer)?;
        return Ok(());
    }
    if let Some(d) = &indexer.definition {
        let object = d
            .as_object()
            .ok_or_else(|| anyhow!("Adapter definition must be a JSON object"))?;
        let allowed: &[&str] = match indexer.kind.as_str() {
            "json" => &[
                "items",
                "title",
                "download",
                "details",
                "size",
                "seeders",
                "peers",
                "query_param",
            ],
            "html" => &[
                "rows",
                "title",
                "download",
                "details",
                "size",
                "seeders",
                "query_param",
            ],
            "unit3d" => &["categories", SINGLE_FILE_TITLE, KEYWORDS_REPLACE_DOTS],
            _ => &["query_param"],
        };
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            bail!("Unknown adapter definition field; Cardigann syntax is not supported");
        }
        if d.get(SINGLE_FILE_TITLE).is_some_and(|v| !v.is_boolean()) {
            bail!("UNIT3D single_file_release_use_filename must be a boolean");
        }
        if d.get(KEYWORDS_REPLACE_DOTS)
            .is_some_and(|v| !v.is_boolean())
        {
            bail!("UNIT3D keywords_replace_dots must be a boolean");
        }
        if let Some(categories) = d.get("categories") {
            let map = categories
                .as_object()
                .ok_or_else(|| anyhow!("Category mapping must be an object"))?;
            for (key, ids) in map {
                if key.parse::<u32>().is_err()
                    || !ids.as_array().is_some_and(|ids| {
                        ids.iter().all(|v| {
                            v.as_u64().is_some()
                                || v.as_str().is_some_and(|s| s.parse::<u32>().is_ok())
                        })
                    })
                {
                    bail!(
                        "Category mappings require numeric category keys and arrays of tracker IDs"
                    );
                }
            }
        }
    }
    if indexer.api_key.contains(['\r', '\n']) || indexer.cookie.contains(['\r', '\n']) {
        bail!("Credentials contain invalid header characters");
    }
    match indexer.kind.as_str() {
        "rss" | "torznab" | "unit3d" => (),
        "json" => {
            let d = definition(indexer)?;
            if !cfg(d, "items").is_empty() && !cfg(d, "items").starts_with('/') {
                bail!("JSON items must be a JSON pointer");
            }
            for key in ["title", "download"] {
                if !cfg(d, key).starts_with('/') {
                    bail!("JSON title and download must be JSON pointers");
                }
            }
        }
        "html" => {
            let d = definition(indexer)?;
            for key in ["rows", "title", "download"] {
                if cfg(d, key).is_empty() {
                    bail!("HTML rows, title and download selectors are required");
                }
                selector(cfg(d, key))?;
            }
            for key in ["details", "size", "seeders"] {
                if !cfg(d, key).is_empty() {
                    selector(cfg(d, key))?;
                }
            }
        }
        _ => bail!(
            "Unsupported adapter kind; Cardigann and native C# definitions are not executable"
        ),
    }
    Ok(())
}
pub fn catalog() -> Value {
    let mut templates = json!([
        {"id":"custom-rss","name":"Custom RSS / Atom","kind":"rss","url":"","description":"Torrent enclosure feeds; configurable query parameter.","language":"und","type":"custom","categories":[]},
        {"id":"custom-torznab","name":"Torznab / existing Jackett","kind":"torznab","url":"","description":"Connect a Torznab endpoint, including an existing Jackett indexer.","language":"und","type":"custom","categories":[]},
        {"id":"custom-unit3d","name":"UNIT3D API","kind":"unit3d","url":"","description":"Modern UNIT3D API with Bearer authentication and data attributes.","language":"und","type":"custom","categories":[]},
        {"id":"custom-json","name":"Custom JSON","kind":"json","url":"","description":"Explicit JSON pointer mapping; not Cardigann.","language":"und","type":"custom","categories":[]},
        {"id":"custom-html","name":"Custom HTML","kind":"html","url":"","description":"Explicit CSS mapping; not Cardigann.","language":"und","type":"custom","categories":[]}
    ]);
    templates
        .as_array_mut()
        .unwrap()
        .extend(crate::builtin::catalog().iter().cloned());
    templates
}

fn replace_params(url: &mut Url, changes: &[(String, String)]) {
    let old: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| !changes.iter().any(|(key, _)| key == k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    let mut pairs = url.query_pairs_mut();
    pairs.extend_pairs(old);
    pairs.extend_pairs(changes.iter().map(|(k, v)| (k, v)));
}
fn request_url(indexer: &Indexer, query: &SearchQuery) -> Result<Url> {
    let mut url = http_url(&indexer.url)?;
    let mut params = Vec::new();
    match indexer.kind.as_str() {
        "torznab" => {
            for key in [
                "t",
                "q",
                "cat",
                "limit",
                "offset",
                "extended",
                "season",
                "ep",
                "imdbid",
                "tmdbid",
                "tvdbid",
                "tvmazeid",
                "rid",
                "traktid",
                "doubanid",
                "year",
                "genre",
                "artist",
                "album",
                "label",
                "track",
                "author",
                "title",
                "publisher",
            ] {
                if !query.get(key).is_empty() {
                    params.push((key.into(), query.get(key).into()));
                }
            }
            if query.get("t").is_empty() {
                params.push(("t".into(), "search".into()));
            }
            // Never forward the management client's apikey.
            params.push(("apikey".into(), indexer.api_key.clone()));
        }
        "unit3d" => {
            let path = url.path().trim_end_matches('/');
            if !path.ends_with("/api/torrents/filter") {
                let path = format!("{}/api/torrents/filter", path);
                url.set_path(&path);
            }
            let mut name = query.keywords();
            if indexer
                .definition
                .as_ref()
                .and_then(|d| d.get(KEYWORDS_REPLACE_DOTS))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                name = name.replace('.', " ");
            }
            params.push(("name".into(), name));
            for (from, to) in [
                ("season", "seasonNumber"),
                ("ep", "episodeNumber"),
                ("imdbid", "imdbId"),
                ("tmdbid", "tmdbId"),
                ("tvdbid", "tvdbId"),
            ] {
                if !query.get(from).is_empty() {
                    params.push((to.into(), query.get(from).trim_start_matches("tt").into()));
                }
            }
            let limit = query
                .get("limit")
                .parse::<u32>()
                .unwrap_or(100)
                .clamp(1, 100);
            let offset = query.get("offset").parse::<u32>().unwrap_or(0);
            params.extend([
                ("perPage".into(), limit.to_string()),
                ("page".into(), (offset / limit + 1).to_string()),
                ("sortField".into(), "created_at".into()),
                ("sortDirection".into(), "desc".into()),
            ]);
            // UNIT3D categories are tracker IDs: use an explicit standard-to-tracker mapping.
            if let Some(map) = indexer
                .definition
                .as_ref()
                .and_then(|d| d.get("categories"))
            {
                for cat in query.get("cat").split(',') {
                    if let Some(ids) = map.get(cat).and_then(Value::as_array) {
                        for id in ids {
                            if let Some(id) = scalar(id) {
                                params.push(("categories[]".into(), id));
                            }
                        }
                    }
                }
            }
        }
        _ => {
            let key = indexer
                .definition
                .as_ref()
                .map(|d| cfg(d, "query_param"))
                .filter(|s| !s.is_empty())
                .unwrap_or("q");
            if !query.keywords().is_empty() {
                params.push((key.into(), query.keywords()));
            }
        }
    }
    replace_params(&mut url, &params);
    Ok(url)
}
pub async fn search(
    client: &reqwest::Client,
    indexer: &Indexer,
    query: &SearchQuery,
) -> Result<Vec<Release>> {
    validate(indexer)?;
    let url = request_url(indexer, query)?;
    let mut request = client.get(url.clone()).header(
        reqwest::header::ACCEPT,
        "application/json, application/rss+xml, application/atom+xml, text/html, */*",
    );
    if !indexer.cookie.is_empty() {
        request = request.header(reqwest::header::COOKIE, &indexer.cookie);
    }
    if indexer.kind == "unit3d" && !indexer.api_key.is_empty() {
        request = request.bearer_auth(&indexer.api_key);
    }
    let response = request
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("Upstream request failed")?;
    let response = response
        .error_for_status()
        .map_err(reqwest::Error::without_url)
        .context("Upstream returned an HTTP error")?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY as u64)
    {
        bail!("Upstream response exceeds 8 MiB limit");
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(reqwest::Error::without_url)
            .context("Unable to read upstream response")?;
        if bytes.len() + chunk.len() > MAX_BODY {
            bail!("Upstream response exceeds 8 MiB limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow!("Adapter requires UTF-8 response encoding"))?;
    let mut releases = match indexer.kind.as_str() {
        "rss" | "torznab" => parse_feed(text, indexer, &url)?,
        "unit3d" => parse_unit3d(text, indexer, &url)?,
        "json" => parse_json(text, indexer, &url)?,
        "html" => parse_html(text, indexer, &url)?,
        _ => unreachable!(),
    };
    if indexer.kind == "rss" {
        let terms: Vec<_> = query
            .keywords()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        releases.retain(|r| {
            let title = r.title.to_lowercase();
            terms.iter().all(|term| title.contains(term))
        });
    }
    if let Ok(limit) = query.get("limit").parse::<usize>() {
        if limit > 0 {
            releases.truncate(limit.min(1000));
        }
    }
    Ok(releases)
}
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
fn number(v: Option<&Value>) -> Option<u64> {
    v.and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
}
fn date(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw)
        .or_else(|_| DateTime::parse_from_rfc2822(raw))
        .ok()
        .map(|d| {
            d.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
}
fn resolved(base: &Url, raw: &str, magnet: bool) -> Option<String> {
    let url = base.join(raw).ok()?;
    if magnet
        && url.scheme() == "magnet"
        && url
            .query_pairs()
            .any(|(k, v)| k == "xt" && v.starts_with("urn:btih:"))
    {
        return Some(url.to_string());
    }
    if http_url(url.as_str()).is_ok() {
        Some(url.to_string())
    } else {
        None
    }
}
fn release(indexer: &Indexer, title: String, link: String) -> Release {
    Release {
        guid: link.clone(),
        title,
        link,
        indexer_id: indexer.id.clone(),
        indexer: indexer.name.clone(),
        categories: indexer.categories.clone(),
        ..Default::default()
    }
}
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .and_then(|n| n.text())
        .map(str::to_owned)
}
fn parse_feed(text: &str, indexer: &Indexer, base: &Url) -> Result<Vec<Release>> {
    let doc = roxmltree::Document::parse(text).map_err(|_| anyhow!("Invalid XML feed"))?;
    if doc
        .descendants()
        .any(|n| n.is_element() && n.tag_name().name() == "error")
    {
        bail!("Upstream feed reported an error");
    }
    if !matches!(doc.root_element().tag_name().name(), "rss" | "feed" | "RDF") {
        bail!("Response is not an RSS or Atom feed");
    }
    let mut results = Vec::new();
    for item in doc
        .descendants()
        .filter(|n| n.is_element() && matches!(n.tag_name().name(), "item" | "entry"))
    {
        let title = child_text(item, "title").unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let enclosure = item.children().find(|n| {
            n.is_element()
                && (n.tag_name().name() == "enclosure"
                    || (n.tag_name().name() == "link" && n.attribute("rel") == Some("enclosure")))
                && n.attribute("type")
                    .map(|t| t.contains("bittorrent") || t == "application/octet-stream")
                    .unwrap_or(true)
        });
        let attrs: std::collections::BTreeMap<String, String> = item
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "attr")
            .filter_map(|n| {
                Some((
                    n.attribute("name")?.to_owned(),
                    n.attribute("value")?.to_owned(),
                ))
            })
            .collect();
        let raw = enclosure
            .and_then(|n| n.attribute("url").or_else(|| n.attribute("href")))
            .map(str::to_owned)
            .or_else(|| attrs.get("magneturl").cloned())
            .or_else(|| child_text(item, "magnetURI"))
            .or_else(|| child_text(item, "link").filter(|s| s.starts_with("magnet:")));
        let Some(link) = raw.and_then(|r| resolved(base, &r, true)) else {
            continue;
        };
        let mut r = release(indexer, title, link);
        r.guid = child_text(item, "guid")
            .or_else(|| child_text(item, "id"))
            .unwrap_or_else(|| r.link.clone());
        let details = child_text(item, "link").or_else(|| {
            item.children()
                .find(|n| {
                    n.is_element()
                        && n.tag_name().name() == "link"
                        && n.attribute("rel").unwrap_or("alternate") == "alternate"
                })
                .and_then(|n| n.attribute("href"))
                .map(str::to_owned)
        });
        r.details = details.and_then(|d| resolved(base, &d, false));
        r.size = enclosure
            .and_then(|n| n.attribute("length"))
            .and_then(|s| s.parse().ok())
            .or_else(|| attrs.get("size").and_then(|s| s.parse().ok()));
        r.seeders = attrs
            .get("seeders")
            .and_then(|s| s.parse().ok())
            .or_else(|| child_text(item, "seeders").and_then(|s| s.parse().ok()));
        r.peers = attrs.get("peers").and_then(|s| s.parse().ok()).or_else(|| {
            attrs
                .get("leechers")
                .and_then(|s| s.parse::<u64>().ok())
                .map(|n| n.saturating_add(r.seeders.unwrap_or(0)))
        });
        r.published = child_text(item, "pubDate")
            .or_else(|| child_text(item, "published"))
            .or_else(|| child_text(item, "updated"))
            .and_then(|s| date(&s));
        let cats: Vec<u32> = item
            .children()
            .filter(|n| {
                n.is_element()
                    && n.tag_name().name() == "attr"
                    && n.attribute("name") == Some("category")
            })
            .filter_map(|n| n.attribute("value")?.parse().ok())
            .collect();
        if !cats.is_empty() {
            r.categories = cats;
        }
        r.attributes = attrs;
        results.push(r);
    }
    Ok(results)
}
fn parse_unit3d(text: &str, indexer: &Indexer, base: &Url) -> Result<Vec<Release>> {
    let doc: Value =
        serde_json::from_str(text).map_err(|_| anyhow!("Invalid UNIT3D JSON response"))?;
    let rows = doc
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("UNIT3D response has no data array"))?;
    let use_filename = indexer
        .definition
        .as_ref()
        .and_then(|d| d.get(SINGLE_FILE_TITLE))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut results = Vec::new();
    for row in rows {
        let a = row.get("attributes").unwrap_or(row);
        // `name` stays required, as the definition's non-optional title_optional field.
        let Some(name) = a.get("name").and_then(Value::as_str) else {
            continue;
        };
        let title = use_filename
            .then(|| single_file_name(a))
            .flatten()
            .unwrap_or(name);
        let Some(link) = a
            .get("download_link")
            .and_then(Value::as_str)
            .and_then(|s| resolved(base, s, true))
        else {
            continue;
        };
        let mut r = release(indexer, title.into(), link);
        r.details = a
            .get("details_link")
            .and_then(Value::as_str)
            .and_then(|s| resolved(base, s, false));
        r.guid = r.details.clone().unwrap_or_else(|| r.link.clone());
        r.size = number(a.get("size"));
        r.seeders = number(a.get("seeders"));
        r.peers = number(a.get("leechers")).map(|n| n.saturating_add(r.seeders.unwrap_or(0)));
        r.published = a.get("created_at").and_then(Value::as_str).and_then(date);
        let featured = a.get("featured").and_then(Value::as_bool).unwrap_or(false);
        let free = a.get("freeleech").and_then(scalar).unwrap_or_default();
        let fraction = free
            .trim_end_matches('%')
            .parse::<f64>()
            .ok()
            .map(|n| n.clamp(0., 100.) / 100.)
            .unwrap_or(0.);
        r.attributes.insert(
            "downloadvolumefactor".into(),
            if featured {
                "0".into()
            } else {
                (1. - fraction).to_string()
            },
        );
        r.attributes.insert(
            "uploadvolumefactor".into(),
            if featured
                || a.get("double_upload")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                "2".into()
            } else {
                "1".into()
            },
        );
        for (from, to) in [
            ("imdb_id", "imdb"),
            ("tmdb_id", "tmdbid"),
            ("tvdb_id", "tvdbid"),
            ("times_completed", "grabs"),
            ("num_file", "files"),
        ] {
            if let Some(value) = a.get(from).and_then(scalar) {
                r.attributes.insert(to.into(), value);
            }
        }
        // An explicit map is authoritative. Unknown or missing tracker IDs remain
        // uncategorized, rather than inheriting every advertised source category.
        if let Some(map) = indexer
            .definition
            .as_ref()
            .and_then(|d| d.get("categories"))
            .and_then(Value::as_object)
        {
            r.categories.clear();
            if let Some(id) = a.get("category_id").and_then(scalar) {
                r.categories = map
                    .iter()
                    .filter(|(_, ids)| {
                        ids.as_array().is_some_and(|ids| {
                            ids.iter()
                                .any(|v| scalar(v).as_deref() == Some(id.as_str()))
                        })
                    })
                    .filter_map(|(k, _)| k.parse().ok())
                    .collect();
            }
        }
        results.push(r);
    }
    if !rows.is_empty() && results.is_empty() {
        bail!("UNIT3D rows contain no valid release attributes");
    }
    Ok(results)
}
/// Conservative subset of 0dayfiles-api.yml: accept numeric 1 or exact string "1".
/// Unlike upstream CoerceLong, do not normalize other count strings. The optional
/// string `files[0].name`, beside `name` in the same row, must not be blank.
fn single_file_name(a: &Value) -> Option<&str> {
    let single = match a.get("num_file")? {
        Value::Number(n) => n.as_f64() == Some(1.),
        Value::String(s) => s == "1",
        _ => false,
    };
    let name = a.get("files")?.get(0)?.get("name")?.as_str()?;
    (single && !name.trim().is_empty()).then_some(name)
}
fn parse_json(text: &str, indexer: &Indexer, base: &Url) -> Result<Vec<Release>> {
    let doc: Value = serde_json::from_str(text).map_err(|_| anyhow!("Invalid JSON response"))?;
    let d = definition(indexer)?;
    let rows = doc
        .pointer(cfg(d, "items"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("JSON items pointer does not identify an array"))?;
    let mut results = Vec::new();
    for row in rows {
        let Some(title) = row.pointer(cfg(d, "title")).and_then(Value::as_str) else {
            continue;
        };
        let Some(link) = row
            .pointer(cfg(d, "download"))
            .and_then(Value::as_str)
            .and_then(|s| resolved(base, s, true))
        else {
            continue;
        };
        let mut r = release(indexer, title.into(), link);
        if !cfg(d, "details").is_empty() {
            r.details = row
                .pointer(cfg(d, "details"))
                .and_then(Value::as_str)
                .and_then(|s| resolved(base, s, false));
        }
        for key in ["size", "seeders", "peers"] {
            if !cfg(d, key).is_empty() {
                let n = number(row.pointer(cfg(d, key)));
                match key {
                    "size" => r.size = n,
                    "seeders" => r.seeders = n,
                    _ => r.peers = n,
                }
            }
        }
        results.push(r);
    }
    Ok(results)
}
fn parse_html(text: &str, indexer: &Indexer, base: &Url) -> Result<Vec<Release>> {
    let d = definition(indexer)?;
    let doc = Html::parse_document(text);
    let rows = selector(cfg(d, "rows"))?;
    let title = selector(cfg(d, "title"))?;
    let download = selector(cfg(d, "download"))?;
    let mut results = Vec::new();
    for row in doc.select(&rows) {
        let Some(name) = row
            .select(&title)
            .next()
            .map(|n| n.text().collect::<String>().trim().to_owned())
        else {
            continue;
        };
        let Some(link) = row
            .select(&download)
            .next()
            .and_then(|n| n.value().attr("href"))
            .and_then(|s| resolved(base, s, true))
        else {
            continue;
        };
        let mut r = release(indexer, name, link);
        for key in ["details", "size", "seeders"] {
            if cfg(d, key).is_empty() {
                continue;
            }
            let sel = selector(cfg(d, key))?;
            if let Some(n) = row.select(&sel).next() {
                match key {
                    "details" => {
                        r.details = n
                            .value()
                            .attr("href")
                            .and_then(|s| resolved(base, s, false))
                    }
                    "size" => r.size = n.text().collect::<String>().trim().parse().ok(),
                    _ => r.seeders = n.text().collect::<String>().trim().parse().ok(),
                }
            }
        }
        results.push(r);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn indexer(kind: &str) -> Indexer {
        Indexer {
            id: "test".into(),
            name: "Test".into(),
            kind: kind.into(),
            url: "https://example.org/".into(),
            ..Default::default()
        }
    }
    #[test]
    fn rss_enclosures_and_metadata() {
        let i = indexer("rss");
        let rows = parse_feed(
            include_str!("../tests/fixtures/adapters/rss.xml"),
            &i,
            &http_url(&i.url).unwrap(),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Example & Linux");
        assert_eq!(rows[0].size, Some(12345));
        assert_eq!(rows[0].categories, vec![7000, 7010]);
        assert_eq!(rows[0].peers, Some(6));
        assert!(rows[0].published.is_some());
    }
    #[test]
    fn atom_magnet_and_unsafe_links() {
        let i = indexer("rss");
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>Magnet</title><link rel="enclosure" type="application/x-bittorrent" href="magnet:?xt=urn:btih:abcdef"/></entry><entry><title>Unsafe</title><link rel="enclosure" href="javascript:alert(1)"/></entry></feed>"#;
        let rows = parse_feed(xml, &i, &http_url(&i.url).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].link.starts_with("magnet:"));
    }
    #[test]
    fn modern_unit3d_percentages_and_categories() {
        let mut i = indexer("unit3d");
        i.definition = Some(json!({"categories":{"2000":[7],"5000":[8]}}));
        let rows = parse_unit3d(
            include_str!("../tests/fixtures/adapters/unit3d.json"),
            &i,
            &http_url(&i.url).unwrap(),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].attributes["downloadvolumefactor"], "0.25");
        assert_eq!(rows[0].attributes["uploadvolumefactor"], "2");
        assert_eq!(rows[0].categories, vec![2000]);
        assert_eq!(rows[0].peers, Some(13));
        assert_eq!(rows[1].attributes["downloadvolumefactor"], "0");
    }
    #[test]
    fn torznab_query_credentials_encoded_and_replaced() {
        let mut i = indexer("torznab");
        i.url = "https://example.org/api?t=old&apikey=stale".into();
        i.api_key = "private & key".into();
        let mut q = SearchQuery::default();
        q.params.extend([
            ("q".into(), "foo & bar".into()),
            ("apikey".into(), "management-secret".into()),
            ("publisher".into(), "Publisher One".into()),
        ]);
        let url = request_url(&i, &q).unwrap();
        let p: Vec<_> = url.query_pairs().collect();
        assert_eq!(p.iter().filter(|(k, _)| k == "apikey").count(), 1);
        assert!(p.iter().any(|(k, v)| k == "apikey" && v == "private & key"));
        assert!(p.iter().any(|(k, v)| k == "q" && v == "foo & bar"));
        assert!(p
            .iter()
            .any(|(k, v)| k == "publisher" && v == "Publisher One"));
        assert!(!url.as_str().contains("management-secret"));
    }
    #[test]
    fn unit3d_query_mapping_and_ids() {
        let mut i = indexer("unit3d");
        i.definition = Some(json!({"categories":{"2000":[7,9]}}));
        let mut q = SearchQuery::default();
        q.params.extend([
            ("cat".into(), "2000".into()),
            ("imdbid".into(), "tt12345".into()),
            ("limit".into(), "50".into()),
            ("offset".into(), "100".into()),
        ]);
        let url = request_url(&i, &q).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(url.path(), "/api/torrents/filter");
        assert_eq!(pairs.iter().filter(|(k, _)| k == "categories[]").count(), 2);
        assert!(pairs.iter().any(|(k, v)| k == "page" && v == "3"));
        assert!(pairs.iter().any(|(k, v)| k == "imdbId" && v == "12345"));
    }
    #[test]
    fn declarative_json_and_html() {
        let mut i = indexer("json");
        i.definition =
            Some(json!({"items":"/items","title":"/name","download":"/torrent","size":"/bytes"}));
        validate(&i).unwrap();
        let base = http_url(&i.url).unwrap();
        let rows = parse_json(
            r#"{"items":[{"name":"Example","torrent":"/file.torrent","bytes":123}]}"#,
            &i,
            &base,
        )
        .unwrap();
        assert_eq!(rows[0].size, Some(123));
        i.kind = "html".into();
        i.definition = Some(json!({"rows":"article","title":"h2","download":"a.torrent"}));
        validate(&i).unwrap();
        let rows = parse_html(
            "<article><h2>Example</h2><a class=torrent href=/file.torrent>Get</a></article>",
            &i,
            &base,
        )
        .unwrap();
        assert_eq!(rows[0].title, "Example");
    }
    #[test]
    fn validation_rejects_unsupported_and_unsafe() {
        for url in ["file:///etc/passwd", "https://user:password@example.org/"] {
            let mut i = indexer("rss");
            i.url = url.into();
            assert!(validate(&i).is_err());
        }
        let mut i = indexer("cardigann");
        assert!(validate(&i).is_err());
        i.kind = "json".into();
        assert!(validate(&i).is_err());
    }
    #[tokio::test]
    async fn mock_http_receives_only_adapter_credentials() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0; 8192];
            let n = socket.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = include_str!("../tests/fixtures/adapters/unit3d.json");
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            request
        });
        let mut i = indexer("unit3d");
        i.url = format!("http://{address}");
        i.api_key = "adapter-token".into();
        i.cookie = "session=private".into();
        let rows = search(&reqwest::Client::new(), &i, &SearchQuery::default())
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let request = handle.join().unwrap().to_lowercase();
        assert!(request.contains("authorization: bearer adapter-token"));
        assert!(request.contains("cookie: session=private"));
        assert!(request.starts_with("get /api/torrents/filter?"));
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;
    fn indexer(kind: &str) -> Indexer {
        Indexer {
            kind: kind.into(),
            url: "https://example.org".into(),
            ..Default::default()
        }
    }
    #[test]
    fn dates_normalize_to_utc_for_engine_sorting() {
        assert_eq!(
            date("2026-10-27T15:00:00+03:00"),
            Some("2026-10-27T12:00:00Z".into())
        );
        assert_eq!(
            date("Tue, 27 Oct 2026 12:00:00 +0000"),
            Some("2026-10-27T12:00:00Z".into())
        );
        assert!(date("nonsense").is_none());
    }
    #[test]
    fn malformed_feed_and_wrong_schema_are_errors() {
        let i = indexer("rss");
        let base = http_url(&i.url).unwrap();
        assert!(parse_feed("<rss>", &i, &base).is_err());
        assert!(parse_feed("<html><body>Login</body></html>", &i, &base).is_err());
        let i = indexer("unit3d");
        for body in [
            "not json",
            r#"{"error":"unauthorized"}"#,
            r#"{"data":[{"unexpected":true}]}"#,
        ] {
            assert!(parse_unit3d(body, &i, &base).is_err());
        }
        assert!(parse_unit3d(r#"{"data":[]}"#, &i, &base)
            .unwrap()
            .is_empty());
    }
    async fn server_response(headers: &str, body: &str, kind: &str) -> Result<Vec<Release>> {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headers = headers.to_owned();
        let body = body.to_owned();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0; 8192];
            let _ = socket.read(&mut buf);
            let _ = write!(socket, "{headers}\r\nConnection: close\r\n\r\n{body}");
        });
        let mut i = indexer(kind);
        i.url = format!("http://{address}/?secret=do-not-expose");
        let result = search(&reqwest::Client::new(), &i, &SearchQuery::default()).await;
        handle.join().unwrap();
        result
    }
    #[tokio::test]
    async fn rejects_declared_and_streaming_oversize() {
        let e = server_response("HTTP/1.1 200 OK\r\nContent-Length: 8388609", "", "rss")
            .await
            .unwrap_err();
        assert!(e.to_string().contains("8 MiB"));
        let body = "x".repeat(MAX_BODY + 1);
        let e = server_response("HTTP/1.1 200 OK", &body, "rss")
            .await
            .unwrap_err();
        assert!(e.to_string().contains("8 MiB"));
    }
    #[tokio::test]
    async fn status_error_preserves_retry_type_without_url_secret() {
        let error = server_response(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0",
            "",
            "rss",
        )
        .await
        .unwrap_err();
        assert!(!format!("{error:#}").contains("do-not-expose"));
        assert!(error.chain().any(|e| e
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.status() == Some(reqwest::StatusCode::SERVICE_UNAVAILABLE))));
    }
    #[tokio::test]
    async fn rss_filters_keywords_locally() {
        let body = include_str!("../tests/fixtures/adapters/rss.xml");
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let mut i = indexer("rss");
        i.url = format!("http://{address}");
        let mut q = SearchQuery::default();
        q.params.insert("artist".into(), "unmatched".into());
        assert!(search(&reqwest::Client::new(), &i, &q)
            .await
            .unwrap()
            .is_empty());
        handle.join().unwrap();
    }
}
