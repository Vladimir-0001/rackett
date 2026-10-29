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
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

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

fn xml(text: &str) -> String {
    text.chars()
        .filter(|c| {
            matches!(*c, '\t' | '\n' | '\r') || (*c >= ' ' && *c != '\u{fffe}' && *c != '\u{ffff}')
        })
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn document(body: String, status: StatusCode) -> Response {
    (
        status,
        [
            ("content-type", "application/xml; charset=utf-8"),
            ("cache-control", "no-store"),
        ],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{body}"),
    )
        .into_response()
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
    format!("urn:rackett:{:x}", hash.finalize())
}
fn public_details(value: Option<&str>) -> Option<String> {
    let url = url::Url::parse(value?).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.query().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}
fn public_release(release: &Release, key: &str, base_url: &str) -> Release {
    let mut out = release.clone();
    out.guid = public_guid(release);
    out.link = ticket_link(release, key, base_url);
    out.details = public_details(release.details.as_deref());
    out.attributes.retain(|k, _| {
        [
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
        ]
        .contains(&k.as_str())
    });
    out
}
/// Produce client-facing links without mutating internal/cache records.
pub fn present_results(response: &mut crate::models::SearchResponse, key: &str, base: &str) {
    response.results = response
        .results
        .iter()
        .map(|r| public_release(r, key, base))
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
    let response = state
        .engine
        .search_selected(
            selected.iter().map(|x| x.id.clone()).collect(),
            SearchQuery { params },
            id == "all",
        )
        .await;
    if response.results.is_empty() && !response.errors.is_empty() && response.pending.is_empty() {
        return error(
            StatusCode::BAD_GATEWAY,
            900,
            "All selected indexers failed; inspect Activity for source status",
        );
    }
    let total = response.results.len();
    // Sources still searching when `maxwait` elapsed; a client may poll again for them.
    let pending = response
        .pending
        .iter()
        .map(|p| {
            format!(
                "<rackett:pending id=\"{}\" name=\"{}\"/>",
                xml(&p.indexer_id),
                xml(&p.indexer)
            )
        })
        .collect::<String>();
    let body = response
        .results
        .iter()
        .skip(offset)
        .take(limit)
        .map(|r| release_xml(r, &settings.api_key, &settings.base_url()))
        .collect::<String>();
    feed(offset, total, pending + &body)
}
fn feed(offset: usize, total: usize, body: String) -> Response {
    document(format!("<rss version=\"2.0\" xmlns:torznab=\"http://torznab.com/schemas/2015/feed\" xmlns:newznab=\"http://www.newznab.com/DTD/2010/feeds/attributes/\" xmlns:rackett=\"urn:rackett:feed\"><channel><title>Rackett</title><description>Native indexer search</description><newznab:response offset=\"{offset}\" total=\"{total}\"/>{body}</channel></rss>"),StatusCode::OK)
}

struct Ticket {
    indexer: String,
    /// Runtime identity of the producing source; downloads reject any other runtime.
    epoch: u64,
    url: String,
    name: String,
    created: Instant,
}
const TICKET_TTL: Duration = Duration::from_secs(7200);
const TICKET_CAPACITY: usize = 4096;

/// Tickets are immutable and created under one lock in monotonic time order.
/// Keep that order explicitly: issuing a cached feed must not scan the entire
/// table for each release. Each queued token is removed at most once.
#[derive(Default)]
struct TicketStore {
    entries: HashMap<String, Ticket>,
    order: VecDeque<String>,
}
impl TicketStore {
    fn issue(&mut self, ticket: Ticket) -> String {
        while let Some(oldest) = self.order.front() {
            if self.entries.len() < TICKET_CAPACITY
                && ticket.created.duration_since(self.entries[oldest].created) < TICKET_TTL
            {
                break;
            }
            let oldest = self.order.pop_front().expect("front exists");
            self.entries.remove(&oldest);
        }
        // Never replace an existing token, even in the event of an RNG collision.
        let token = loop {
            let token = crate::store::random_key();
            if !self.entries.contains_key(&token) {
                break token;
            }
        };
        self.order.push_back(token.clone());
        self.entries.insert(token.clone(), ticket);
        token
    }

    fn get(&self, token: &str, now: Instant) -> Option<&Ticket> {
        self.entries
            .get(token)
            .filter(|ticket| now.duration_since(ticket.created) < TICKET_TTL)
    }
}
static TICKETS: OnceLock<Mutex<TicketStore>> = OnceLock::new();
fn ticket_link(release: &Release, key: &str, base_url: &str) -> String {
    if release.link.starts_with("magnet:?") {
        return release.link.clone();
    }
    let mut tickets = TICKETS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let token = tickets.issue(Ticket {
        indexer: release.indexer_id.clone(),
        epoch: release.source_epoch,
        url: release.link.clone(),
        name: release.title.clone(),
        created: Instant::now(),
    });
    // Never derive feed URLs from untrusted Host or forwarded headers.
    format!("{base_url}/dl/{token}?apikey={key}")
}
fn release_xml(release: &Release, key: &str, base_url: &str) -> String {
    let public = public_release(release, key, base_url);
    let release = &public;
    let link = &release.link;
    let mut attrs = String::new();
    let mut attr = |k: &str, v: &str| {
        attrs.push_str(&format!(
            "<torznab:attr name=\"{}\" value=\"{}\"/>",
            xml(k),
            xml(v)
        ))
    };
    if let Some(v) = release.seeders {
        attr("seeders", &v.to_string());
    }
    if let Some(v) = release.peers {
        attr("peers", &v.to_string());
    }
    if let Some(v) = release.size {
        attr("size", &v.to_string());
    }
    for cat in &release.categories {
        attr("category", &cat.to_string());
    }
    for (k, v) in &release.attributes {
        if !["seeders", "peers", "size", "category"].contains(&k.as_str()) {
            attr(k, v);
        }
    }
    let date = release
        .published
        .as_ref()
        .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
        .map(|x| format!("<pubDate>{}</pubDate>", x.to_rfc2822()))
        .unwrap_or_default();
    let details = release
        .details
        .as_ref()
        .map(|x| format!("<comments>{}</comments>", xml(x)))
        .unwrap_or_default();
    format!("<item><title>{}</title><guid isPermaLink=\"false\">{}</guid><link>{}</link><enclosure url=\"{}\" length=\"{}\" type=\"application/x-bittorrent\"/>{date}{details}<jackettindexer id=\"{}\">{}</jackettindexer>{attrs}</item>",xml(&release.title),xml(&release.guid),xml(link),xml(link),release.size.unwrap_or(0),xml(&release.indexer_id),xml(&release.indexer))
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
    let movies=response.results.iter().map(|r|json!({"release_name":format!("{} [{}]",r.title,r.indexer),"torrent_id":public_guid(r),"details_url":public_details(r.details.as_deref()),"download_url":ticket_link(r,&settings.api_key,&settings.base_url()),"size":r.size.map(|s|s/1024/1024).unwrap_or(0),"seeders":r.seeders.map(|x|x as i64).unwrap_or(-1),"leechers":r.peers.zip(r.seeders).map(|(p,s)|p.saturating_sub(s) as i64).unwrap_or(-1),"publish_date":r.published})).collect::<Vec<_>>();
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
    let found = TICKETS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&ticket, Instant::now())
        .map(|t| (t.indexer.clone(), t.epoch, t.url.clone(), t.name.clone()));
    let Some((id, epoch, url, name)) = found else {
        return error(StatusCode::NOT_FOUND, 201, "Download expired; search again");
    };
    match state.engine.download(&id, epoch, &url).await {
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
        Err(e) if e.is::<crate::engine::DownloadExpired>() => {
            error(StatusCode::NOT_FOUND, 201, "Download expired; search again")
        }
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

    fn fixture_ticket(created: Instant, n: usize) -> Ticket {
        Ticket {
            indexer: "fixture-source".into(),
            epoch: 42,
            url: format!("https://fixture.invalid/download/{n}"),
            name: format!("Fixture release {n}"),
            created,
        }
    }

    #[test]
    fn tickets_expire_without_needing_another_search() {
        let mut store = TicketStore::default();
        let start = Instant::now();
        let first = store.issue(fixture_ticket(start, 1));
        let second = store.issue(fixture_ticket(start + Duration::from_secs(1), 2));
        assert!(store
            .get(&first, start + TICKET_TTL - Duration::from_nanos(1))
            .is_some());
        assert!(store.get(&first, start + TICKET_TTL).is_none());
        assert!(store.get(&second, start + TICKET_TTL).is_some());
        let third = store.issue(fixture_ticket(start + TICKET_TTL, 3));
        assert!(!store.entries.contains_key(&first));
        assert_eq!(store.order.iter().collect::<Vec<_>>(), [&second, &third]);
        // An idle period can expire the entire queue. Both indexes remain bounded.
        let fourth = store.issue(fixture_ticket(start + TICKET_TTL * 2, 4));
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.order.len(), 1);
        assert!(store.get(&fourth, start + TICKET_TTL * 2).is_some());
    }

    #[test]
    fn tickets_evict_oldest_and_preserve_opaque_runtime_bound_targets() {
        let mut store = TicketStore::default();
        let start = Instant::now();
        let tokens: Vec<_> = (0..TICKET_CAPACITY)
            .map(|n| store.issue(fixture_ticket(start + Duration::from_nanos(n as u64), n)))
            .collect();
        for (n, token) in tokens.iter().enumerate() {
            let ticket = store.get(token, start + Duration::from_secs(1)).unwrap();
            assert_eq!(ticket.indexer, "fixture-source");
            assert_eq!(ticket.epoch, 42);
            assert_eq!(ticket.url, format!("https://fixture.invalid/download/{n}"));
            assert_eq!(ticket.name, format!("Fixture release {n}"));
            assert_eq!(token.len(), 64);
            assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        let newest = store.issue(fixture_ticket(start + Duration::from_secs(1), 0));
        assert!(!tokens.contains(&newest));
        assert!(store
            .get(&tokens[0], start + Duration::from_secs(1))
            .is_none());
        assert!(store
            .get(&tokens[1], start + Duration::from_secs(1))
            .is_some());
        assert_eq!(store.entries.len(), TICKET_CAPACITY);
        assert_eq!(store.order.len(), TICKET_CAPACITY);
        assert_eq!(store.order.back(), Some(&newest));
    }

    #[test]
    #[ignore = "Manual timing comparison; no tracker or service traffic"]
    fn ticket_issuance_benchmark() {
        use std::hint::black_box;
        // The previous production algorithm, including its per-entry clock reads.
        fn legacy_issue(entries: &mut HashMap<String, Ticket>, n: usize) {
            entries.retain(|_, ticket| ticket.created.elapsed() < TICKET_TTL);
            if entries.len() >= TICKET_CAPACITY {
                let oldest = entries
                    .iter()
                    .min_by_key(|(_, ticket)| ticket.created)
                    .map(|(key, _)| key.clone())
                    .unwrap();
                entries.remove(&oldest);
            }
            entries.insert(
                crate::store::random_key(),
                fixture_ticket(Instant::now(), n),
            );
        }
        let mut old_samples = Vec::new();
        let mut new_samples = Vec::new();
        for round in 0..6 {
            let mut old = HashMap::new();
            let mut new = TicketStore::default();
            for n in 0..TICKET_CAPACITY {
                old.insert(
                    crate::store::random_key(),
                    fixture_ticket(Instant::now(), n),
                );
                new.issue(fixture_ticket(Instant::now(), n));
            }
            // Alternate order; 100 cached feeds with 20 HTTP links apiece.
            for candidate in [round % 2 == 0, round % 2 != 0] {
                let start = Instant::now();
                for n in 0..2000 {
                    if candidate {
                        black_box(new.issue(fixture_ticket(Instant::now(), n)));
                    } else {
                        legacy_issue(&mut old, n);
                    }
                }
                let per_feed_us = start.elapsed().as_secs_f64() * 1e6 / 100.0;
                if candidate {
                    new_samples.push(per_feed_us);
                } else {
                    old_samples.push(per_feed_us);
                }
            }
            assert_eq!(old.len(), TICKET_CAPACITY);
            assert_eq!(new.entries.len(), TICKET_CAPACITY);
            assert_eq!(new.order.len(), TICKET_CAPACITY);
        }
        println!("ticket benchmark: microseconds per 20-link feed; previous={old_samples:?}; candidate={new_samples:?}");
    }
    #[test]
    fn escapes_xml_and_removes_illegal_controls() {
        assert_eq!(xml("a\u{0}&\"<é>"), "a&amp;&quot;&lt;é&gt;");
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
        let rendered = release_xml(&r, "secret", "http://127.0.0.1:9117");
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
