//! Cardigann `download.selectors` resolution (pinned `CardigannIndexer.cs:2083-2185`).
//!
//! A ticketed release link is a detail page. Each declared selector is tried in order
//! against that page: the first element it matches yields its attribute (or text),
//! then the selector's filters. The value is resolved against the page URL. A magnet
//! is returned as-is; another link is downloaded and, unless `testlinktorrent: false`,
//! must start like a bencoded torrent (`d`) or the next selector is tried.
//!
//! `infohash` (`:2112-2140`) instead selects a hash and a title on the page and returns
//! `MagnetUtil.InfoHashToPublicMagnet(hash, title)`.
//!
//! Admitted subset: an optional `before` and at most one of `selectors` or `infohash`.
//! Selector text may hold templates (`MatchSelector`, `:2050`). Search headers are sent
//! with every download request of a public source (`:2094`). `method` blocks,
//! `pathselector` and per-selector `usebeforeresponse` stay blocked.
use crate::{
    cardigann::Node,
    cardigann_exec::{
        css, filters::Filter, request::Charset, selector::Selector, template::Context,
    },
};
use anyhow::{anyhow, bail, Result};

const MAX_SELECTORS: usize = 8;
const MAX_FILTERS: usize = 32;
const MAX_PAGE: usize = 8 * 1024 * 1024;

/// `MagnetUtil._Trackers` (pinned `Utils/MagnetUtil.cs:12-24`), in order.
const PUBLIC_TRACKERS: [&str; 10] = [
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://vito-tracker.space:6969/announce",
    "udp://vito-tracker.duckdns.org:6969/announce",
    "udp://tracker.theoks.net:6969/announce",
    "udp://tracker.srv00.com:6969/announce",
    "udp://tracker.qu.ax:6969/announce",
    "udp://tracker.corpscorp.online:80/announce",
];
/// `InfoHashToPublicMagnet`: blank inputs yield no magnet. Every value is
/// `WebUtilityHelpers.UrlEncode(_, UTF8)`.
pub fn public_magnet(hash: &str, title: &str) -> Option<String> {
    if hash.trim().is_empty() || title.trim().is_empty() {
        return None;
    }
    let encode = crate::cardigann_exec::filters::url_encode;
    let mut magnet = format!("magnet:?xt=urn:btih:{hash}&dn={}", encode(title));
    for tracker in PUBLIC_TRACKERS {
        magnet.push_str("&tr=");
        magnet.push_str(&encode(tracker));
    }
    Some(magnet)
}
type Field = (Selector, Vec<Filter>);
/// `download.before` (`HandleRequestAsync`, `CardigannIndexer.cs:1971-2011`): one GET
/// or POST, e.g. a "thanks" form, sent with the link as Referer; its response is
/// ignored. Templates see `.DownloadUri.*` from the release link and `.Config.*`.
struct Before {
    path: crate::cardigann_exec::template::Template,
    post: bool,
    inputs: Vec<(String, crate::cardigann_exec::template::Template)>,
}
/// A rendered `before` request: a path (with any GET query) and POST fields.
pub struct BeforeRequest {
    pub path: String,
    pub fields: Option<Vec<(String, String)>>,
}
pub struct DownloadSelectors {
    before: Option<Before>,
    /// `infohash.usebeforeresponse` with a `before` block (`:2118`): the hash and title
    /// are selected on the `before` response instead of the release link.
    before_page: bool,
    /// `ParseCustomHeaders(Download.Headers ?? Search.Headers, variables)` (`:2094`,
    /// `:2192-2207`): the first value of each search header, as a template.
    headers: Vec<(String, crate::cardigann_exec::template::Template)>,
    charset: Charset,
    selectors: Vec<Field>,
    /// `download.infohash` hash and title fields; exclusive with `selectors`.
    infohash: Option<[Field; 2]>,
    /// `testlinktorrent` (`IndexerDefinition.cs:44`, default true).
    pub test_link: bool,
}

fn keys(node: &Node) -> Result<Vec<&str>> {
    node.pairs()
        .ok_or_else(|| anyhow!("Download block must be a mapping"))?
        .iter()
        .map(|(k, _)| {
            k.text()
                .ok_or_else(|| anyhow!("Download key must be scalar"))
        })
        .collect()
}

fn compile_field(item: &Node, charset: Charset) -> Result<Field> {
    let names = keys(item)?;
    if names
        .iter()
        .any(|k| !["selector", "attribute", "filters"].contains(k))
        || names.iter().filter(|k| **k == "selector").count() != 1
        || names.iter().filter(|k| **k == "attribute").count() > 1
        || names.iter().filter(|k| **k == "filters").count() > 1
    {
        bail!("Unsupported download selector property");
    }
    let text = item
        .string("selector")
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(|| anyhow!("Unsupported download selector text"))?;
    if item
        .string("attribute")
        .is_some_and(|a| a.is_empty() || a.contains("{{"))
    {
        bail!("Unsupported download selector attribute");
    }
    if text.contains("{{") {
        // `MatchSelector` renders the selector with the download variables (`:2050`);
        // the rendered text is parsed when a page is matched.
        crate::cardigann_exec::template::Template::compile(text).map_err(|e| anyhow!(e))?;
    } else if !text.starts_with(":root") {
        css::Css::parse(text).map_err(|e| anyhow!(e))?;
    }
    let selector = Selector::compile(item).map_err(|e| anyhow!(e))?;
    selector.validate_html().map_err(|e| anyhow!(e))?;
    let mut filters = Vec::new();
    for f in item
        .get("filters")
        .map(|f| f.items().ok_or_else(|| anyhow!("Filters must be a list")))
        .transpose()?
        .unwrap_or_default()
    {
        if keys(f)?.iter().any(|k| !["name", "args"].contains(k)) {
            bail!("Unsupported download filter property");
        }
        let args: Vec<String> = match f.get("args") {
            Some(n) => match n.items() {
                Some(v) => v
                    .iter()
                    .map(|v| v.text().unwrap_or_default().to_owned())
                    .collect(),
                None => vec![n.text().unwrap_or_default().to_owned()],
            },
            None => Vec::new(),
        };
        if filters.len() >= MAX_FILTERS {
            bail!("Download filter count limit exceeded");
        }
        filters.push(
            Filter::compile_for(f.string("name").unwrap_or_default(), &args, charset)
                .map_err(|e| anyhow!(e))?,
        );
    }
    Ok((selector, filters))
}

/// `ParseCustomHeaders` over `search.headers`, as the search plan compiles them.
fn compile_headers(
    root: &Node,
) -> Result<Vec<(String, crate::cardigann_exec::template::Template)>> {
    let mut headers = Vec::new();
    let Some(node) = root.get("search").and_then(|s| s.get("headers")) else {
        return Ok(headers);
    };
    for (k, v) in node
        .pairs()
        .ok_or_else(|| anyhow!("Search headers must be a mapping"))?
    {
        let name = k
            .text()
            .filter(|k| {
                !k.is_empty()
                    && k.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
            })
            .ok_or_else(|| anyhow!("Invalid search header name"))?;
        // Upstream sends only the first value of each header.
        let value = v
            .items()
            .and_then(|v| v.first())
            .and_then(Node::text)
            .ok_or_else(|| anyhow!("Invalid search header value"))?;
        if headers.len() >= 32 {
            bail!("Search header limit exceeded");
        }
        headers.push((
            name.to_owned(),
            crate::cardigann_exec::template::Template::compile(value).map_err(|e| anyhow!(e))?,
        ));
    }
    Ok(headers)
}

fn compile_before(node: &Node) -> Result<Before> {
    use crate::cardigann_exec::template::Template;
    let names = keys(node)?;
    if names
        .iter()
        .any(|k| !["path", "method", "inputs"].contains(k))
    {
        bail!("Unsupported download before property");
    }
    let template = |s: &str| {
        if s.len() > 8192 || s.chars().any(char::is_control) {
            bail!("Unsupported download before template");
        }
        Template::compile(s).map_err(|e| anyhow!(e))
    };
    let path = template(
        node.string("path")
            .filter(|p| !p.is_empty())
            .ok_or_else(|| anyhow!("Download before needs a path"))?,
    )?;
    let post = match node.string("method") {
        None => false,
        Some(m) if m.eq_ignore_ascii_case("get") => false,
        Some(m) if m.eq_ignore_ascii_case("post") => true,
        Some(_) => bail!("Unsupported download before method"),
    };
    let mut inputs = Vec::new();
    for (k, v) in node.get("inputs").and_then(Node::pairs).unwrap_or_default() {
        let name = k
            .text()
            .filter(|n| !n.is_empty() && n.len() <= 256 && !n.chars().any(char::is_control))
            .ok_or_else(|| anyhow!("Invalid download before input"))?;
        let value = v
            .text()
            .ok_or_else(|| anyhow!("Invalid download before input"))?;
        if inputs.len() >= 32 {
            bail!("Download before input limit exceeded");
        }
        inputs.push((name.to_owned(), template(value)?));
    }
    Ok(Before { path, post, inputs })
}
/// `AddTemplateVariablesFromUri(variables, link, ".DownloadUri")` (`:2030-2046`).
pub fn add_download_uri(ctx: &mut Context, link: &url::Url) {
    use serde_json::Value;
    let query = link.query().map(|q| format!("?{q}")).unwrap_or_default();
    let path_and_query = format!("{}{query}", link.path());
    let port = link
        .port_or_known_default()
        .map(|p| p.to_string())
        .unwrap_or_default();
    for (k, v) in [
        ("AbsoluteUri", link.as_str().to_owned()),
        ("AbsolutePath", link.path().to_owned()),
        ("Scheme", link.scheme().to_owned()),
        ("Host", link.host_str().unwrap_or_default().to_owned()),
        ("Port", port),
        ("PathAndQuery", path_and_query),
        ("Query", query),
    ] {
        ctx.values
            .insert(format!(".DownloadUri.{k}"), Value::String(v));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (k, v) in link.query_pairs() {
        // The first value of a repeated key wins, as upstream.
        if seen.insert(k.to_string()) {
            ctx.values.insert(
                format!(".DownloadUri.Query.{k}"),
                Value::String(v.into_owned()),
            );
        }
    }
}

impl DownloadSelectors {
    /// Whether a page must be fetched and selected after any `before` request; false
    /// means upstream downloads the release link itself.
    pub fn selects(&self) -> bool {
        self.infohash.is_some() || !self.selectors.is_empty()
    }
    /// Whether the infohash is selected on the `before` response, not the release link.
    pub fn selects_before_response(&self) -> bool {
        self.before_page
    }
    /// The rendered search headers for every download request (`:2094`). Only public
    /// definitions declare them here; login sessions never send custom headers.
    pub fn headers(&self, ctx: &Context) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        for (name, template) in &self.headers {
            let value = template.render(ctx)?;
            if value.len() > 8192 || value.chars().any(char::is_control) {
                return Err("Invalid download header value".into());
            }
            out.push((name.clone(), value));
        }
        Ok(out)
    }
    /// The rendered `before` request for one release link, if declared. GET inputs join
    /// the path query (`GetQueryString(Encoding)`); POST inputs become form fields.
    pub fn before_request(&self, ctx: &Context) -> Result<Option<BeforeRequest>, String> {
        let Some(before) = &self.before else {
            return Ok(None);
        };
        let mut path = before.path.render(ctx)?;
        let mut fields = Vec::new();
        for (name, value) in &before.inputs {
            fields.push((name.clone(), value.render(ctx)?));
        }
        if before.post {
            return Ok(Some(BeforeRequest {
                path,
                fields: Some(fields),
            }));
        }
        if !fields.is_empty() {
            path.push(if path.contains('?') { '&' } else { '?' });
            let encoded: Result<Vec<_>, String> = fields
                .iter()
                .map(|(k, v)| Ok(format!("{k}={}", self.charset.url_encode(v)?)))
                .collect();
            path.push_str(&encoded?.join("&"));
        }
        Ok(Some(BeforeRequest { path, fields: None }))
    }
    /// The admitted `download: {selectors: [...]}` shape, or an error naming why not.
    pub fn compile(root: &Node) -> Result<Self> {
        let download = root
            .get("download")
            .ok_or_else(|| anyhow!("No download block"))?;
        let mut mode = keys(download)?;
        mode.sort_unstable();
        if ![
            &["selectors"][..],
            &["infohash"],
            &["before", "selectors"],
            &["before", "infohash"],
            &["before"],
        ]
        .contains(&mode.as_slice())
        {
            bail!("Only download before, selectors or infohash are supported");
        }
        // Public sources send download requests through the plain transport, which carries
        // headers and returns the `before` response. Login sessions do neither.
        let public = root.get("login").is_none();
        let headers = compile_headers(root)?;
        if !headers.is_empty() && !public {
            bail!("Download requests would carry search headers");
        }
        let charset = Charset::from_definition(root).map_err(|e| anyhow!(e))?;
        let before = download.get("before").map(compile_before).transpose()?;
        if public
            && before
                .as_ref()
                .is_some_and(|b| b.post || !headers.is_empty())
        {
            bail!("Unsupported public download before request");
        }
        let mut selectors = Vec::new();
        let mut infohash = None;
        let mut before_page = false;
        if download.get("selectors").is_none() && download.get("infohash").is_none() {
            // `before` alone: upstream then downloads the release link itself.
        } else if let Some(block) = download.get("infohash") {
            let mut names = keys(block)?;
            names.sort_unstable();
            if names != ["hash", "title"] && names != ["hash", "title", "usebeforeresponse"] {
                bail!("Unsupported download infohash property");
            }
            let use_before = match block.get("usebeforeresponse").map(Node::text) {
                None | Some(Some("false")) => false,
                Some(Some("true")) => true,
                _ => bail!("Unsupported usebeforeresponse value"),
            };
            // Without a `before` block upstream fetches the release link (`:2118`).
            before_page = use_before && before.is_some();
            if before_page && !public {
                bail!("Download before response needs a public source");
            }
            infohash = Some([
                compile_field(block.get("hash").unwrap(), charset)?,
                compile_field(block.get("title").unwrap(), charset)?,
            ]);
        } else {
            let items = download
                .get("selectors")
                .and_then(Node::items)
                .filter(|items| (1..=MAX_SELECTORS).contains(&items.len()))
                .ok_or_else(|| anyhow!("Unsupported download selector list"))?;
            for item in items {
                selectors.push(compile_field(item, charset)?);
            }
        }
        let test_link = match root.get("testlinktorrent") {
            None => true,
            Some(n) => match n.text() {
                Some("true") => true,
                Some("false") => false,
                _ => bail!("Unsupported testlinktorrent value"),
            },
        };
        Ok(Self {
            before,
            before_page,
            headers,
            charset,
            selectors,
            infohash,
            test_link,
        })
    }
    /// `MatchSelector` for every selector over one fetched page, in declaration order.
    /// `None` is a selector that did not match or failed (upstream catches the exception
    /// and continues with the next one, `:2175-2180`). Upstream refetches the page per
    /// selector; one fetch is shared here. `ctx` holds the `.DownloadUri` variables.
    pub fn candidates(&self, page: &str, ctx: &Context) -> Result<Vec<Option<String>>, String> {
        if page.len() > MAX_PAGE {
            return Err("Download page exceeds byte budget".into());
        }
        let html = css::parse_document(page);
        css::check_error_document(&html)?;
        let root = html.root_element();
        let render = |s: &str| crate::cardigann_exec::template::Template::compile(s)?.render(ctx);
        let eval = |(selector, filters): &Field| -> Result<Option<String>, String> {
            // An attribute the matched element lacks is an upstream exception, which
            // the selector loop also treats as "try the next selector".
            let Some(mut value) = selector.extract_html(root, &render)? else {
                return Ok(None);
            };
            for filter in filters {
                value = filter.apply(&value, ctx)?;
            }
            Ok(Some(value))
        };
        if let Some([hash, title]) = &self.infohash {
            // A missing hash or title is an upstream exception; the release then has
            // no usable download.
            return Ok(vec![match (eval(hash)?, eval(title)?) {
                (Some(h), Some(t)) => public_magnet(&h, &t),
                _ => None,
            }]);
        }
        Ok(self
            .selectors
            .iter()
            .map(|field| eval(field).unwrap_or(None))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{builtin, cardigann};
    fn root(id: &str) -> Node {
        let raw = builtin::SOURCES.iter().find(|(i, _)| *i == id).unwrap().1;
        cardigann::parse(raw, cardigann::Limits::default()).unwrap()
    }
    fn ctx(id: &str) -> Context {
        let plan = crate::cardigann_exec::Plan::compile(&root(id)).unwrap();
        plan.context(
            &Default::default(),
            &crate::models::SearchQuery::default(),
            chrono::Utc::now(),
        )
    }
    #[test]
    fn pinned_selector_blocks_compile_and_others_stay_blocked() {
        for id in [
            "chdbits",
            "crackingpatching",
            "hellashut",
            "u2",
            "dimeadozen",
        ] {
            assert!(DownloadSelectors::compile(&root(id)).is_ok(), "{id}");
        }
        assert!(
            !DownloadSelectors::compile(&root("hellashut"))
                .unwrap()
                .test_link
        );
        assert!(
            DownloadSelectors::compile(&root("chdbits"))
                .unwrap()
                .test_link
        );
        for id in [
            "0magnet",
            "torrentcore",
            "torrenttip",
            "torrentsome",
            // Templated selectors, public search headers, public before response.
            "limetorrents",
            "torrentdownloads",
            "catorrent",
            "damagnet",
            "skidrowrepack",
            "magnetdownload",
        ] {
            assert!(DownloadSelectors::compile(&root(id)).is_ok(), "{id}");
        }
        // pathselector and method blocks; search headers on a login source.
        for id in ["ildragonero", "hdsky", "acgrip"] {
            assert!(DownloadSelectors::compile(&root(id)).is_err(), "{id}");
        }
    }
    #[test]
    fn templated_selectors_render_settings_and_failures_try_the_next() {
        let s = DownloadSelectors::compile(&root("limetorrents")).unwrap();
        let page = "<a class=csprite_dltorrent href='http://itorrents.org/torrent/X.torrent'>t</a>\
                    <a class=csprite_dltorrent href='magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567'>m</a>";
        // Defaults: primary `magnet:`, fallback `//itorrents.` (rewritten by the filter).
        assert_eq!(
            s.candidates(page, &ctx("limetorrents")).unwrap(),
            [
                Some("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into()),
                Some("https://itorrents.net/torrent/X.torrent".into()),
            ]
        );
        let mut c = ctx("limetorrents");
        c.values.insert(
            ".Config.primarydownloadlink".into(),
            serde_json::Value::String("\"]".into()),
        );
        // An unparsable rendered selector is an upstream exception: next selector.
        assert_eq!(
            s.candidates(page, &c).unwrap(),
            [None, Some("https://itorrents.net/torrent/X.torrent".into())]
        );
    }
    #[test]
    fn public_search_headers_render_for_download_requests() {
        let s = DownloadSelectors::compile(&root("catorrent")).unwrap();
        let mut c = ctx("catorrent");
        c.values.insert(
            ".Config.sitelink".into(),
            serde_json::Value::String("https://site.test/".into()),
        );
        assert_eq!(
            s.headers(&c).unwrap(),
            [("Referer".to_owned(), "https://site.test/".to_owned())]
        );
        assert!(DownloadSelectors::compile(&root("0magnet"))
            .unwrap()
            .headers(&c)
            .unwrap()
            .is_empty());
        // A login source would need its session to send them: still blocked.
        let src = builtin::SOURCES
            .iter()
            .find(|(i, _)| *i == "catorrent")
            .unwrap()
            .1
            .replace("\nsearch:\n", "\nlogin:\n  method: cookie\n\nsearch:\n");
        let r = cardigann::parse(&src, cardigann::Limits::default()).unwrap();
        assert!(DownloadSelectors::compile(&r).is_err());
    }
    #[test]
    fn infohash_reads_the_public_before_response() {
        let s = DownloadSelectors::compile(&root("magnetdownload")).unwrap();
        assert!(s.selects_before_response());
        let mut c = ctx("magnetdownload");
        add_download_uri(
            &mut c,
            &url::Url::parse("https://site.test/info/14152933").unwrap(),
        );
        let before = s.before_request(&c).unwrap().unwrap();
        assert_eq!(before.path, "api/json_info?hashes=14152933");
        assert!(before.fields.is_none());
        // The JSON reply parsed as HTML; `:root` text is the whole body.
        let json = r#"{"0123456789ABCDEF0123456789abcdef01234567": {"name": "Fixture Release", "size": 1}}"#;
        let magnet = s.candidates(json, &c).unwrap()[0].clone().unwrap();
        assert!(magnet.starts_with(
            "magnet:?xt=urn:btih:0123456789ABCDEF0123456789abcdef01234567&dn=Fixture+Release&tr="
        ));
        // Without a before block the release link is fetched instead (`:2118`).
        assert!(!DownloadSelectors::compile(&root("damagnet"))
            .unwrap()
            .selects_before_response());
    }
    #[test]
    fn first_match_in_document_order_per_selector() {
        let s = DownloadSelectors::compile(&root("hellashut")).unwrap();
        let page =
            "<p><a href='magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567'>m</a>\
                    <a href='download.php?id=7'>d</a></p>";
        // A selector group matches the first element in document order, as AngleSharp.
        assert_eq!(
            s.candidates(page, &ctx("hellashut")).unwrap(),
            [Some(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into()
            )]
        );
        let s = DownloadSelectors::compile(&root("chdbits")).unwrap();
        assert_eq!(
            s.candidates("<p>none</p>", &ctx("chdbits")).unwrap(),
            [None]
        );
        assert_eq!(
            s.candidates("<a href='download.php?id=9&amp;x=1'>d</a>", &ctx("chdbits"))
                .unwrap(),
            [Some("download.php?id=9&x=1".into())]
        );
        assert!(s
            .candidates(&"x".repeat(MAX_PAGE + 1), &ctx("chdbits"))
            .is_err());
    }
    #[test]
    fn infohash_builds_the_pinned_public_magnet() {
        let s = DownloadSelectors::compile(&root("torrentsome")).unwrap();
        let page =
            "<a href='magnet:?xt=urn:btih:0123456789ABCDEF0123456789abcdef01234567&dn=x'>m</a>\
                    <h1 class=text-base>  Fixture Release  </h1>";
        let magnet = s.candidates(page, &ctx("torrentsome")).unwrap()[0]
            .clone()
            .unwrap();
        // dn and every tracker use WebUtility encoding ('+' for space, upper-case hex).
        assert!(magnet.starts_with(
            "magnet:?xt=urn:btih:0123456789ABCDEF0123456789abcdef01234567&dn=Fixture+Release&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce"
        ));
        assert!(magnet.ends_with("&tr=udp%3A%2F%2Ftracker.corpscorp.online%3A80%2Fannounce"));
        assert_eq!(magnet.matches("&tr=").count(), 10);
        assert!(crate::builtin_runtime::detail_magnet_uri(&magnet).is_some());
        // A missing title yields no magnet.
        let hash_only =
            "<a href='magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567'>m</a>";
        assert_eq!(
            s.candidates(hash_only, &ctx("torrentsome")).unwrap(),
            [None]
        );
        assert_eq!(public_magnet(" ", "t"), None);
    }
    #[test]
    fn before_blocks_render_download_uri_variables() {
        for id in [
            "arabafenice",
            "arabscene",
            "drugari",
            "letseed",
            "tmghub",
            "torrent-heaven",
        ] {
            assert!(DownloadSelectors::compile(&root(id)).is_ok(), "{id}");
        }
        // pathselector and usebeforeresponse stay unsupported.
        for id in ["ildragonero", "kinozal-magnet"] {
            assert!(DownloadSelectors::compile(&root(id)).is_err(), "{id}");
        }
        let s = DownloadSelectors::compile(&root("arabafenice")).unwrap();
        assert!(s.selects());
        let mut c = ctx("arabafenice");
        add_download_uri(
            &mut c,
            &url::Url::parse("https://site.test/details.php?id=42&x=1&id=7").unwrap(),
        );
        let before = s.before_request(&c).unwrap().unwrap();
        assert_eq!(before.path, "thanks.php");
        let fields = before.fields.unwrap();
        assert!(fields.contains(&("infohash".into(), "42".into())));
        assert!(fields.contains(&("thanks".into(), "1".into())));
        // A before-only block downloads the link itself.
        assert!(!DownloadSelectors::compile(&root("letseed"))
            .unwrap()
            .selects());
    }
}
