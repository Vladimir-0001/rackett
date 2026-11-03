//! Torznab and TorrentPotato compatibility over the native Rust search engine.
use crate::{
    models::{Indexer, Release, SearchQuery},
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2.0/indexers/{id}/results/torznab", get(torznab))
        .route("/api/v2.0/indexers/{id}/results/torznab/api", get(torznab))
        .route("/api/v2.0/indexers/{id}/results/potato", get(potato))
        .route("/api/v2.0/indexers/{id}/results/potato/api", get(potato))
        .route("/torznab/{id}", get(torznab))
        .route("/torznab/{id}/api", get(torznab))
        .route("/potato/{id}", get(potato))
        .route("/dl/{ticket}", get(download))
}

/// Appends `text` as XML text or attribute content in one pass: characters XML 1.0
/// cannot carry (C0 controls other than tab, LF and CR; U+FFFE; U+FFFF) are dropped
/// and the five markup characters are escaped. Everything to escape or drop starts
/// with an ASCII byte or 0xEF, so the scan is bytewise and slices on char boundaries.
fn push_xml(out: &mut String, text: &str) {
    // Bytes that may need escaping or dropping; everything else is copied as is.
    const SPECIAL: [bool; 256] = {
        let mut table = [false; 256];
        let mut b = 0;
        while b < 0x20 {
            table[b] = !matches!(b as u8, b'\t' | b'\n' | b'\r');
            b += 1;
        }
        table[b'&' as usize] = true;
        table[b'<' as usize] = true;
        table[b'>' as usize] = true;
        table[b'"' as usize] = true;
        table[b'\'' as usize] = true;
        table[0xef] = true;
        table
    };
    let bytes = text.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        if !SPECIAL[usize::from(bytes[i])] {
            i += 1;
            continue;
        }
        let (escaped, width) = match bytes[i] {
            b'&' => ("&amp;", 1),
            b'<' => ("&lt;", 1),
            b'>' => ("&gt;", 1),
            b'"' => ("&quot;", 1),
            b'\'' => ("&apos;", 1),
            b'\t' | b'\n' | b'\r' => {
                i += 1;
                continue;
            }
            0..=0x1f => ("", 1),
            0xef if bytes.get(i + 1) == Some(&0xbf)
                && matches!(bytes.get(i + 2), Some(0xbe | 0xbf)) =>
            {
                ("", 3)
            }
            _ => {
                i += 1;
                continue;
            }
        };
        out.push_str(&text[start..i]);
        out.push_str(escaped);
        i += width;
        start = i;
    }
    out.push_str(&text[start..]);
}
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    push_xml(&mut out, text);
    out
}
const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
/// An XML response whose body already starts with [`DECLARATION`].
fn declared(body: String, status: StatusCode) -> Response {
    (
        status,
        [
            ("content-type", "application/xml; charset=utf-8"),
            ("cache-control", "no-store"),
        ],
        body,
    )
        .into_response()
}
fn document(body: String, status: StatusCode) -> Response {
    declared(DECLARATION.to_owned() + &body, status)
}
fn error(status: StatusCode, code: u32, description: &str) -> Response {
    document(
        format!(
            "<error code=\"{code}\" description=\"{}\"/>",
            xml(description)
        ),
        status,
    )
}
fn equal_secret(a: &str, b: &str) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |diff, (x, y)| diff | (x ^ y))
        == 0
}
fn authorized(params: &BTreeMap<String, String>, headers: &HeaderMap, key: &str) -> bool {
    let supplied = params
        .get("apikey")
        .or_else(|| params.get("passkey"))
        .map(String::as_str)
        .or_else(|| headers.get("x-api-key").and_then(|h| h.to_str().ok()))
        .unwrap_or("");
    equal_secret(supplied, key)
}

pub(crate) fn validate_query(params: &BTreeMap<String, String>) -> Result<(usize, usize), String> {
    for field in [
        "offset", "limit", "season", "tvdbid", "tmdbid", "tvmazeid", "rid", "traktid", "doubanid",
        "year",
    ] {
        if let Some(v) = params.get(field) {
            if !v.is_empty() && v.parse::<u32>().is_err() {
                return Err(format!("Invalid {field}: expected a nonnegative integer"));
            }
        }
    }
    if let Some(categories) = params.get("cat") {
        for c in categories.split(',').filter(|c| !c.is_empty()) {
            c.parse::<u32>()
                .map_err(|_| "Invalid category".to_string())?;
        }
    }
    if let Some(id) = params.get("imdbid") {
        let digits = id.strip_prefix("tt").unwrap_or(id);
        if digits.len() < 5 || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid imdbid".into());
        }
    }
    let offset = params
        .get("offset")
        .and_then(|x| x.parse().ok())
        .unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(|x| x.parse().ok())
        .unwrap_or(100usize)
        .clamp(1, 500);
    if offset > 100_000 {
        return Err("Offset exceeds result budget".into());
    }
    Ok((offset, limit))
}

/// Maps a Torznab `t` value to its caps mode name; unknown values fall back to `search`.
fn caps_mode(t: &str) -> &'static str {
    match t {
        "search" => "search",
        "tvsearch" => "tv-search",
        "movie" => "movie-search",
        "music" => "music-search",
        "book" => "book-search",
        _ => "search",
    }
}
pub(crate) fn validate_source_ids(
    params: &BTreeMap<String, String>,
    selected: &[Indexer],
) -> Result<(), String> {
    let t = params.get("t").map(String::as_str).unwrap_or("search");
    check_sources(params, caps_mode(t), selected)
}
/// Keeps the sources that can serve the query, mirroring Jackett's aggregate
/// `indexers.Where(i => i.CanHandleQuery(query))` instead of failing the whole request.
fn compatible_sources(
    params: &BTreeMap<String, String>,
    mode: &str,
    selected: Vec<Indexer>,
) -> Vec<Indexer> {
    selected
        .into_iter()
        .filter(|i| check_sources(params, mode, std::slice::from_ref(i)).is_ok())
        .collect()
}
fn check_sources(
    params: &BTreeMap<String, String>,
    mode: &str,
    selected: &[Indexer],
) -> Result<(), String> {
    for i in selected.iter().filter(|i| i.kind == "cardigann") {
        let modes = crate::builtin::source_modes(i);
        let allowed = modes
            .get(mode)
            .ok_or_else(|| "Search mode unsupported by selected bundled source".to_owned())?;
        for key in [
            "imdbid",
            "tmdbid",
            "tvdbid",
            "tvmazeid",
            "rid",
            "traktid",
            "doubanid",
            "season",
            "ep",
            "artist",
            "album",
            "author",
            "title",
            "publisher",
        ] {
            if params.get(key).is_some_and(|v| !v.is_empty()) && !allowed.iter().any(|a| a == key) {
                return Err(format!("{key} unsupported by selected bundled source"));
            }
        }
    }
    for field in [
        "imdbid", "tmdbid", "tvdbid", "tvmazeid", "rid", "traktid", "doubanid",
    ] {
        if params.get(field).is_some_and(|v| !v.is_empty())
            && selected.iter().any(|i| {
                i.kind != "torznab"
                    && i.kind != "cardigann"
                    && !(i.kind == "unit3d" && ["imdbid", "tmdbid", "tvdbid"].contains(&field))
            })
        {
            return Err(format!("{field} is unsupported by one or more selected sources; select a compatible source"));
        }
    }
    Ok(())
}
fn public_guid(release: &Release) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(release.indexer_id.as_bytes());
    hash.update([0]);
    hash.update(release.guid.as_bytes());
    "urn:rackett:".to_owned() + &crate::ticket::hex(&hash.finalize())
}
/// Attributes a client may see; the rest can carry source internals.
const PUBLIC_ATTRIBUTES: [&str; 26] = [
    "imdb",
    "imdbid",
    "tmdbid",
    "tvdbid",
    "tvmazeid",
    "rageid",
    "traktid",
    "doubanid",
    "infohash",
    "minimumratio",
    "minimumseedtime",
    "downloadvolumefactor",
    "uploadvolumefactor",
    "files",
    "grabs",
    "year",
    "genre",
    "language",
    "subs",
    "author",
    "booktitle",
    "publisher",
    "artist",
    "album",
    "label",
    "track",
];
fn public_details(value: Option<&str>) -> Option<String> {
    let url = url::Url::parse(value?).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.query().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}
fn public_release(release: &Release, links: &Links) -> Release {
    let mut out = release.clone();
    out.guid = public_guid(release);
    out.link = ticket_link(release, links);
    out.details = public_details(release.details.as_deref());
    out.attributes
        .retain(|k, _| PUBLIC_ATTRIBUTES.contains(&k.as_str()));
    out
}
/// Produce client-facing links without mutating internal/cache records.
pub fn present_results(response: &mut crate::models::SearchResponse, links: &Links) {
    response.results = response
        .results
        .iter()
        .map(|r| public_release(r, links))
        .collect();
}

fn capabilities(indexers: &[Indexer]) -> String {
    // Aggregate dispatch filters incompatible sources, so advertise the union of
    // usable source modes and parameters rather than hiding partially supported modes.
    let mut categories: Vec<u32> = indexers
        .iter()
        .flat_map(crate::builtin::source_categories)
        .collect();
    categories.sort_unstable();
    categories.dedup();
    let cats = categories
        .iter()
        .map(|c| format!("<category id=\"{c}\" name=\"{}\"/>", category_name(*c)))
        .collect::<String>();
    let searching = [
        "search",
        "tv-search",
        "movie-search",
        "music-search",
        "book-search",
    ]
    .into_iter()
    .map(|mode| {
        let mut supported = Vec::new();
        let mut available = false;
        for i in indexers {
            let modes = crate::builtin::source_modes(i);
            if let Some(params) = modes.get(mode) {
                available = true;
                for param in params {
                    if !supported.contains(param) {
                        supported.push(param.clone());
                    }
                }
            }
        }
        format!(
            "<{mode} available=\"{}\" supportedParams=\"{}\"/>",
            if available { "yes" } else { "no" },
            xml(&supported.join(","))
        )
    })
    .collect::<String>();
    format!("<caps><server version=\"{}\" title=\"Rackett\"/><limits max=\"500\" default=\"100\"/><searching>{searching}</searching><categories>{cats}</categories></caps>",env!("CARGO_PKG_VERSION"))
}
fn category_name(id: u32) -> &'static str {
    match id / 1000 {
        1 => "Console",
        2 => "Movies",
        3 => "Audio",
        4 => "PC",
        5 => "TV",
        6 => "XXX",
        7 => "Books",
        _ => "Other",
    }
}

async fn torznab(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(mut params): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let settings = state.engine.settings().await;
    if !authorized(&params, &headers, &settings.api_key) {
        return error(StatusCode::UNAUTHORIZED, 100, "Invalid API key");
    }
    let all = state.engine.indexers().await;
    let selected: Vec<_> = all
        .into_iter()
        .filter(|x| x.enabled && (id == "all" || x.id == id))
        .collect();
    if id != "all" && selected.is_empty() {
        return error(
            StatusCode::NOT_FOUND,
            201,
            "Indexer is not configured or is disabled",
        );
    }
    let mode = params
        .get("t")
        .map(String::as_str)
        .unwrap_or("search")
        .to_ascii_lowercase();
    if mode == "caps" {
        return document(capabilities(&selected), StatusCode::OK);
    }
    if mode == "indexers" && id == "all" {
        let body=selected.iter().map(|x| format!("<indexer id=\"{}\" configured=\"true\"><title>{}</title><type>{}</type>{}</indexer>",xml(&x.id),xml(&x.name),xml(&x.kind),capabilities(std::slice::from_ref(x)))).collect::<String>();
        return document(format!("<indexers>{body}</indexers>"), StatusCode::OK);
    }
    if !["search", "tvsearch", "movie", "music", "book"].contains(&mode.as_str()) {
        return error(StatusCode::BAD_REQUEST, 203, "Search mode is not supported");
    }
    let (offset, limit) = match validate_query(&params) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, 201, &e),
    };
    let source_mode = caps_mode(&mode);
    let selected = if id == "all" {
        compatible_sources(&params, source_mode, selected)
    } else {
        if let Err(message) = check_sources(&params, source_mode, &selected) {
            return error(StatusCode::BAD_REQUEST, 203, &message);
        }
        selected
    };
    // An empty ID list means every enabled source to the engine, so never dispatch it.
    if selected.is_empty() {
        return feed(offset, 0, String::new());
    }
    for key in ["apikey", "passkey", "offset", "limit"] {
        params.remove(key);
    }
    params.insert("t".into(), mode);
    params.insert(
        "limit".into(),
        offset.saturating_add(limit).min(1000).to_string(),
    );
    let (response, results) = state
        .engine
        .search_shared(
            selected.iter().map(|x| x.id.clone()).collect(),
            SearchQuery { params },
            id == "all",
        )
        .await;
    if results.is_empty() && !response.errors.is_empty() && response.pending.is_empty() {
        return error(
            StatusCode::BAD_GATEWAY,
            900,
            "All selected indexers failed; inspect Activity for source status",
        );
    }
    let total = results.len();
    let shown = &results[offset.min(total)..offset.saturating_add(limit).min(total)];
    let links = state
        .engine
        .links(
            shown.iter().map(|r| &**r),
            &settings.api_key,
            &settings.base_url(),
        )
        .await;
    let mut out = feed_start(offset, total, 1024 + shown.len() * 1536);
    // Sources still searching when `maxwait` elapsed; a client may poll again for them.
    for p in &response.pending {
        out.push_str("<rackett:pending id=\"");
        push_xml(&mut out, &p.indexer_id);
        out.push_str("\" name=\"");
        push_xml(&mut out, &p.indexer);
        out.push_str("\"/>");
    }
    for r in shown {
        push_shared_xml(&mut out, r, &links);
    }
    for r in shown {
        r.mark_presented();
    }
    feed_end(out)
}
fn feed_start(offset: usize, total: usize, capacity: usize) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(capacity);
    out.push_str(DECLARATION);
    let _ = write!(out, "<rss version=\"2.0\" xmlns:torznab=\"http://torznab.com/schemas/2015/feed\" xmlns:newznab=\"http://www.newznab.com/DTD/2010/feeds/attributes/\" xmlns:rackett=\"urn:rackett:feed\"><channel><title>Rackett</title><description>Native indexer search</description><newznab:response offset=\"{offset}\" total=\"{total}\"/>");
    out
}
fn feed_end(mut out: String) -> Response {
    out.push_str("</channel></rss>");
    declared(out, StatusCode::OK)
}
fn feed(offset: usize, total: usize, body: String) -> Response {
    let mut out = feed_start(offset, total, body.len() + 1024);
    out.push_str(&body);
    feed_end(out)
}

/// Issues client-facing download links for one presentation (see [`Engine::links`]).
///
/// [`Engine::links`]: crate::engine::Engine::links
pub struct Links {
    key: std::sync::Arc<crate::ticket::TicketKey>,
    /// By runtime epoch, which is process-unique: the source ID and its fingerprint.
    fingerprints: HashMap<u64, (String, [u8; 32])>,
    api_key: String,
    base_url: String,
    /// SHA-256 prefix of the API key and base URL, the rest of a rendered item's context.
    context: [u8; 16],
}
impl Links {
    pub fn new(
        key: std::sync::Arc<crate::ticket::TicketKey>,
        fingerprints: HashMap<(String, u64), [u8; 32]>,
        api_key: &str,
        base_url: &str,
    ) -> Self {
        use sha2::{Digest, Sha256};
        let digest =
            Sha256::digest(serde_json::to_vec(&(api_key, base_url)).expect("strings serialize"));
        let mut context = [0; 16];
        context.copy_from_slice(&digest[..16]);
        Self {
            key,
            fingerprints: fingerprints
                .into_iter()
                .map(|((id, epoch), fingerprint)| (epoch, (id, fingerprint)))
                .collect(),
            api_key: api_key.into(),
            base_url: base_url.into(),
            context,
        }
    }
    /// The credential fingerprint a link to `release` is issued under; a release from a
    /// runtime replaced before presentation gets one that never matches.
    fn fingerprint(&self, release: &Release) -> [u8; 32] {
        self.fingerprints
            .get(&release.source_epoch)
            .filter(|(id, _)| *id == release.indexer_id)
            .map(|(_, fingerprint)| *fingerprint)
            .unwrap_or([0; 32])
    }
}
/// A cached release's rendered Torznab item and the exact context it was rendered in:
/// the API key and base URL digest and the credential fingerprint behind its link.
pub(crate) type Rendered = std::sync::OnceLock<([u8; 16], [u8; 32], Box<str>)>;
/// Magnets are returned as they are. Other links become permanent `/dl/` tokens; a
/// release from a runtime replaced before presentation gets a fingerprint that never
/// matches, and a link too long to seal gets no download URL.
fn ticket_link(release: &Release, links: &Links) -> String {
    if release.link.starts_with("magnet:?") {
        return release.link.clone();
    }
    let fingerprint = links.fingerprint(release);
    let token = links.key.seal(&crate::ticket::Ticket {
        indexer: release.indexer_id.clone(),
        fingerprint,
        url: release.link.clone(),
        name: release.title.clone(),
    });
    let Some(token) = token else {
        return String::new();
    };
    // Never derive feed URLs from untrusted Host or forwarded headers.
    format!("{}/dl/{token}?apikey={}", links.base_url, links.api_key)
}
#[cfg(test)]
fn release_xml(release: &Release, links: &Links) -> String {
    let mut out = String::with_capacity(1024);
    push_release_xml(&mut out, release, links);
    out
}
/// Appends one release as a Torznab item. From its result set's second showing on, the
/// item is rendered at most once per presentation context and then reused.
fn push_shared_xml(out: &mut String, shared: &crate::engine::Shared, links: &Links) {
    let fingerprint = links.fingerprint(shared);
    let slot = shared.rendered();
    if let Some((context, fp, item)) = slot.get() {
        if *context == links.context && *fp == fingerprint {
            out.push_str(item);
            return;
        }
    }
    let start = out.len();
    push_release_xml(out, shared, links);
    if shared.presented() {
        let _ = slot.set((links.context, fingerprint, out[start..].into()));
    }
}
/// `date.to_rfc2822()` written in place. Chrono formats through its generic strftime
/// machinery, which was a visible share of rendering an uncached feed.
fn push_rfc2822(out: &mut String, date: &chrono::DateTime<chrono::FixedOffset>) {
    use chrono::{Datelike, Timelike};
    use std::fmt::Write;
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let year = date.year();
    if !(0..=9999).contains(&year) {
        // Outside RFC 2822's four-digit years; keep chrono's own behaviour.
        out.push_str(&date.to_rfc2822());
        return;
    }
    let offset = date.offset().local_minus_utc();
    let (sign, offset) = if offset < 0 {
        ('-', -offset)
    } else {
        ('+', offset)
    };
    let _ = write!(
        out,
        "{}, {} {} {:04} {:02}:{:02}:{:02} {}{:02}{:02}",
        DAYS[date.weekday().num_days_from_monday() as usize],
        date.day(),
        MONTHS[date.month0() as usize],
        year,
        date.hour(),
        date.minute(),
        // A leap second (nanosecond >= 1e9) prints as :60, as chrono does.
        date.second() + date.nanosecond() / 1_000_000_000,
        sign,
        offset / 3600,
        offset / 60 % 60,
    );
}
/// Decimal formatting into a reused buffer.
struct Digits(String);
fn itoa_buffer() -> Digits {
    Digits(String::with_capacity(20))
}
impl Digits {
    fn format(&mut self, n: u64) -> &str {
        use std::fmt::Write;
        self.0.clear();
        let _ = write!(self.0, "{n}");
        &self.0
    }
}
fn push_release_xml(out: &mut String, release: &Release, links: &Links) {
    use std::fmt::Write;
    let link = ticket_link(release, links);
    out.push_str("<item><title>");
    push_xml(out, &release.title);
    out.push_str("</title><guid isPermaLink=\"false\">");
    out.push_str(&public_guid(release));
    out.push_str("</guid><link>");
    push_xml(out, &link);
    out.push_str("</link><enclosure url=\"");
    push_xml(out, &link);
    let _ = write!(
        out,
        "\" length=\"{}\" type=\"application/x-bittorrent\"/>",
        release.size.unwrap_or(0)
    );
    if let Some(date) = release
        .published
        .as_ref()
        .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
    {
        out.push_str("<pubDate>");
        push_rfc2822(out, &date);
        out.push_str("</pubDate>");
    }
    if let Some(details) = public_details(release.details.as_deref()) {
        out.push_str("<comments>");
        push_xml(out, &details);
        out.push_str("</comments>");
    }
    out.push_str("<jackettindexer id=\"");
    push_xml(out, &release.indexer_id);
    out.push_str("\">");
    push_xml(out, &release.indexer);
    out.push_str("</jackettindexer>");
    let mut attr = |k: &str, v: &str| {
        out.push_str("<torznab:attr name=\"");
        push_xml(out, k);
        out.push_str("\" value=\"");
        push_xml(out, v);
        out.push_str("\"/>");
    };
    let mut number = itoa_buffer();
    if let Some(v) = release.seeders {
        attr("seeders", number.format(v));
    }
    if let Some(v) = release.peers {
        attr("peers", number.format(v));
    }
    if let Some(v) = release.size {
        attr("size", number.format(v));
    }
    for cat in &release.categories {
        attr("category", number.format(u64::from(*cat)));
    }
    for (k, v) in &release.attributes {
        if PUBLIC_ATTRIBUTES.contains(&k.as_str())
            && !["seeders", "peers", "size", "category"].contains(&k.as_str())
        {
            attr(k, v);
        }
    }
    out.push_str("</item>");
}

async fn potato(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(mut params): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let settings = state.engine.settings().await;
    if !authorized(&params, &headers, &settings.api_key) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error":"Invalid API key"})),
        )
            .into_response();
    }
    if id != "all" && state.engine.indexer(&id).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error":"Unknown indexer"})),
        )
            .into_response();
    }
    if let Some(search) = params.remove("search") {
        params.insert("q".into(), search);
    }
    params.remove("apikey");
    params.remove("passkey");
    params.insert("t".into(), "movie".into());
    let response = state.engine.search(vec![id], SearchQuery { params }).await;
    let links = state
        .engine
        .links(&response.results, &settings.api_key, &settings.base_url())
        .await;
    let movies=response.results.iter().map(|r|json!({"release_name":format!("{} [{}]",r.title,r.indexer),"torrent_id":public_guid(r),"details_url":public_details(r.details.as_deref()),"download_url":ticket_link(r,&links),"size":r.size.map(|s|s/1024/1024).unwrap_or(0),"seeders":r.seeders.map(|x|x as i64).unwrap_or(-1),"leechers":r.peers.zip(r.seeders).map(|(p,s)|p.saturating_sub(s) as i64).unwrap_or(-1),"publish_date":r.published})).collect::<Vec<_>>();
    axum::Json(json!({"results":movies,"errors":response.errors})).into_response()
}

async fn download(
    State(state): State<AppState>,
    Path(ticket): Path<String>,
    Query(params): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&params, &headers, &state.engine.settings().await.api_key) {
        return error(StatusCode::UNAUTHORIZED, 100, "Invalid API key");
    }
    let Ok(ticket) = state.engine.ticket_key().open(&ticket) else {
        return error(StatusCode::NOT_FOUND, 201, "Unknown download link");
    };
    let name = ticket.name.clone();
    match state.engine.download_link(&ticket).await {
        Ok(crate::engine::Download::Magnet(magnet)) => match magnet.parse::<HeaderValue>() {
            Ok(location) => (
                StatusCode::FOUND,
                [
                    (header::LOCATION, location),
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                ],
            )
                .into_response(),
            Err(_) => error(
                StatusCode::BAD_GATEWAY,
                900,
                &crate::builtin_runtime::DetailFailure::InvalidMagnet.to_string(),
            ),
        },
        Ok(crate::engine::Download::Torrent { bytes, .. }) => {
            if crate::torrent::validate(&bytes).is_err() {
                return error(
                    StatusCode::BAD_GATEWAY,
                    900,
                    "Source returned invalid torrent data",
                );
            }
            let filename: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
                .take(150)
                .collect();
            let disposition = format!(
                "attachment; filename=\"{}.torrent\"",
                if filename.is_empty() {
                    "download"
                } else {
                    &filename
                }
            );
            (
                [
                    ("content-type", "application/x-bittorrent"),
                    ("content-disposition", disposition.as_str()),
                    ("cache-control", "no-store"),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) if e.is::<crate::engine::DownloadExpired>() => error(
            StatusCode::NOT_FOUND,
            201,
            "Download link retired: the source was removed, disabled or changed; search again",
        ),
        Err(e) if e.is::<crate::builtin_runtime::DetailFailure>() => error(
            StatusCode::BAD_GATEWAY,
            900,
            &e.downcast_ref::<crate::builtin_runtime::DetailFailure>()
                .map(ToString::to_string)
                .unwrap_or_default(),
        ),
        Err(_) => error(
            StatusCode::BAD_GATEWAY,
            900,
            "Download failed; source must permit a same-origin torrent download",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links() -> Links {
        Links::new(
            std::sync::Arc::new(crate::ticket::TicketKey::from_bytes(&[3; 32])),
            HashMap::from([(("custom-1".into(), 7), [9; 32])]),
            "secret",
            "http://127.0.0.1:9117",
        )
    }
    #[test]
    fn links_are_sealed_permanent_and_keep_magnets() {
        let links = links();
        let release = Release {
            indexer_id: "custom-1".into(),
            source_epoch: 7,
            link: "https://tracker.example/dl.php?id=1&passkey=SECRETPASSKEY".into(),
            title: "Fixture".into(),
            ..Default::default()
        };
        let link = ticket_link(&release, &links);
        assert!(link.starts_with("http://127.0.0.1:9117/dl/"), "{link}");
        assert!(link.ends_with("?apikey=secret"));
        assert!(!link.contains("SECRETPASSKEY") && !link.contains("tracker"));
        let token = &link["http://127.0.0.1:9117/dl/".len()..link.find('?').unwrap()];
        let ticket = links.key.open(token).unwrap();
        assert_eq!(ticket.fingerprint, [9; 32]);
        assert_eq!(ticket.url, release.link);
        // A runtime replaced before presentation gets a fingerprint that never matches.
        let stale = Release {
            source_epoch: 6,
            ..release.clone()
        };
        let link = ticket_link(&stale, &links);
        let token = &link["http://127.0.0.1:9117/dl/".len()..link.find('?').unwrap()];
        assert_eq!(links.key.open(token).unwrap().fingerprint, [0; 32]);
        let magnet = Release {
            link: "magnet:?xt=urn:btih:123".into(),
            ..release
        };
        assert_eq!(ticket_link(&magnet, &links), "magnet:?xt=urn:btih:123");
    }
    #[test]
    fn escapes_xml_and_removes_illegal_controls() {
        assert_eq!(xml("a\u{0}&\"<é>"), "a&amp;&quot;&lt;é&gt;");
        assert_eq!(
            xml("\t\n\r'\u{1f}\u{fffe}\u{ffff}\u{fffd}\u{efbf}"),
            "\t\n\r&apos;\u{fffd}\u{efbf}"
        );
        // The previous char-by-char filter, for every char class the scan treats apart.
        let reference = |t: &str| {
            t.chars()
                .filter(|c| {
                    matches!(*c, '\t' | '\n' | '\r')
                        || (*c >= ' ' && *c != '\u{fffe}' && *c != '\u{ffff}')
                })
                .collect::<String>()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;")
        };
        let sample: String = (0u32..0x300)
            .chain(0xfff0..0x10010)
            .filter_map(char::from_u32)
            .collect();
        assert_eq!(xml(&sample), reference(&sample));
    }
    #[test]
    fn rfc2822_matches_chrono() {
        let mut stamp = -62_135_596_800i64; // 0001-01-01
        let mut offsets = [0, 3600, -18000, 19800, -34200, 50400, -43200]
            .into_iter()
            .cycle();
        while stamp < 253_402_300_799 {
            let offset = chrono::FixedOffset::east_opt(offsets.next().unwrap()).unwrap();
            let date = chrono::DateTime::from_timestamp(stamp, 0)
                .unwrap()
                .with_timezone(&offset);
            let mut out = String::new();
            push_rfc2822(&mut out, &date);
            assert_eq!(out, date.to_rfc2822(), "{stamp}");
            stamp += 7_654_321;
        }
        let leap = chrono::DateTime::parse_from_rfc3339("2016-12-31T23:59:60+01:00").unwrap();
        let mut out = String::new();
        push_rfc2822(&mut out, &leap);
        assert_eq!(out, leap.to_rfc2822());
    }
    #[test]
    fn malformed_numbers_are_errors() {
        for field in ["season", "offset", "cat"] {
            assert!(validate_query(&BTreeMap::from([(field.into(), "oops".into())])).is_err());
        }
    }
    #[test]
    fn publisher_does_not_overwrite_author() {
        let q = SearchQuery {
            params: BTreeMap::from([
                ("author".into(), "Author".into()),
                ("publisher".into(), "Publisher".into()),
            ]),
        };
        assert_eq!(q.get("author"), "Author");
        assert_eq!(q.get("publisher"), "Publisher");
        assert!(q.keywords().contains("Author Publisher"));
    }
    #[test]
    fn rendering_never_mutates_cached_release() {
        let r = Release {
            title: "A & B".into(),
            guid: "id".into(),
            link: "magnet:?xt=urn:btih:123&dn=a".into(),
            ..Default::default()
        };
        let before = r.link.clone();
        let rendered = release_xml(&r, &links());
        assert!(rendered.contains("A &amp; B"));
        assert_eq!(r.link, before);
        assert!(!rendered.contains("<pubDate>"));
    }
    #[test]
    fn credentials_require_full_match() {
        assert!(equal_secret("abc", "abc"));
        assert!(!equal_secret("abc", "ab"));
        assert!(!equal_secret("", ""));
        assert!(!equal_secret("abd", "abc"));
    }
    fn source(id: &str, kind: &str, builtin: Option<&str>) -> Indexer {
        Indexer {
            id: id.into(),
            kind: kind.into(),
            definition: builtin.map(|b| serde_json::json!({"builtin_id": b, "settings": {}})),
            ..Default::default()
        }
    }
    fn compatible(query: &[(&str, &str)], sources: &[Indexer]) -> Vec<String> {
        let params: BTreeMap<String, String> = query
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        let t = params.get("t").map(String::as_str).unwrap_or("search");
        compatible_sources(&params, caps_mode(t), sources.to_vec())
            .into_iter()
            .map(|i| i.id)
            .collect()
    }
    #[test]
    fn aggregate_selection_skips_sources_that_cannot_handle_the_query() {
        // EZTV is not runtime-ready; only its bundled caps are consulted here.
        let sources = [
            source("eztv", "cardigann", Some("eztv")),
            source("1337x", "cardigann", Some("1337x")),
            source("feed", "rss", None),
            source("newznab", "torznab", None),
            source("tracker", "unit3d", None),
            source("missing", "cardigann", None),
        ];
        assert_eq!(
            compatible(&[("t", "movie"), ("q", "x")], &sources),
            ["1337x", "feed", "newznab", "tracker"]
        );
        assert_eq!(
            compatible(&[("t", "tvsearch"), ("season", "1"), ("ep", "2")], &sources),
            ["eztv", "1337x", "feed", "newznab", "tracker"]
        );
        assert_eq!(
            compatible(&[("t", "tvsearch"), ("tvdbid", "42")], &sources),
            ["newznab", "tracker"]
        );
        assert_eq!(
            compatible(&[("t", "tvsearch"), ("tvmazeid", "42")], &sources),
            ["newznab"]
        );
        assert_eq!(
            compatible(&[("t", "movie"), ("imdbid", "tt1234567")], &sources[..3]),
            Vec::<String>::new()
        );
        // Each kept source passes the explicit-source check, and each skipped one fails it.
        let params = BTreeMap::from([("t".into(), "movie".into())]);
        for s in &sources {
            let kept = compatible(&[("t", "movie")], std::slice::from_ref(s)).len() == 1;
            assert_eq!(
                kept,
                validate_source_ids(&params, std::slice::from_ref(s)).is_ok()
            );
        }
        assert!(validate_source_ids(&params, &sources).is_err());
    }
}
