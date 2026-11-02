//! Offline immutable Cardigann execution. This module never performs I/O or creates sessions.
mod categories;
pub mod css;
mod dates;
mod dotnet_regex;
pub mod filters;
mod json_scalar;
pub(crate) mod login_form;
pub mod request;
mod rowfilters;
pub mod selector;
mod single_byte;
pub mod template;
mod xml;
use crate::{
    cardigann::{self, Diagnostic, Node, Severity},
    models::{Release, SearchQuery},
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;
use template::{Context, Template};

#[derive(Clone, Debug)]
pub struct ExecutionError {
    pub path: String,
    pub message: String,
}
impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}
impl std::error::Error for ExecutionError {}
fn error(path: impl Into<String>, message: impl Into<String>) -> ExecutionError {
    ExecutionError {
        path: path.into(),
        message: message.into(),
    }
}
/// Transport facts about one response, recorded by the HTTP client rather than derived from
/// the definition, settings or query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponseMeta {
    /// Status of the final response.
    pub status: u16,
    /// Whether any redirect was followed, even one that returned to the original URL.
    pub redirected: bool,
    /// Whether the final response carried a Refresh header (upstream WebResult.IsRedirect).
    pub refresh: bool,
    /// Raw body length before decoding.
    pub body_bytes: usize,
}
#[derive(Clone, Debug)]
pub struct Blocker {
    pub path: String,
    pub feature: String,
}
struct Field {
    name: String,
    selector: selector::Selector,
    filters: Vec<filters::Filter>,
    optional: bool,
    default: Option<Template>,
}
/// Upstream `Rows.Dateheaders` (CardigannIndexer.cs:1910-1946): a selector block read
/// from the rows before an undated row.
struct DateHeaders {
    selector: selector::Selector,
    filters: Vec<filters::Filter>,
    optional: bool,
}
struct Category {
    id: String,
    desc: Option<String>,
    default: bool,
    cats: Vec<u32>,
}
/// Owned, read-only plan; all changing settings/query/result values belong to a call's Context.
pub struct Plan {
    id: String,
    name: String,
    requests: request::RequestPlan,
    fields: Vec<Field>,
    keyword_filters: Vec<filters::Filter>,
    preprocessing_filters: Vec<filters::Filter>,
    errors: Vec<css::Css>,
    rows: Template,
    /// `.Config` keys substituted into `rows`; their settings are never coerced.
    row_values: Vec<String>,
    /// Upstream `Rows.After` (`IndexerDefinition.cs:156`); zero when absent or non-positive.
    after: usize,
    date_headers: Option<DateHeaders>,
    /// JSON `multiple` and `missingAttributeEqualsNoResults`; HTML ignores both, as
    /// upstream does.
    json_shape: selector::JsonRowShape,
    row_filters: rowfilters::RowFilters,
    count: Option<selector::Selector>,
    attribute: Option<String>,
    settings: BTreeMap<String, Value>,
    categories: Vec<Category>,
    blockers: Vec<Blocker>,
}
impl Plan {
    pub fn compile(root: &Node) -> Result<Self, ExecutionError> {
        if !matches!(root.value, cardigann::Value::Mapping(_)) {
            return Err(error("$", "Definition must be a mapping"));
        }
        let diagnostics = cardigann::validate(root);
        if let Some(d) = diagnostics.iter().find(|d| d.severity == Severity::Error) {
            return Err(error(&d.path, &d.message));
        }
        allowed(
            root,
            &[
                "id",
                "replaces",
                "name",
                "description",
                "language",
                "type",
                "encoding",
                "followredirect",
                "testlinktorrent",
                "requestDelay",
                "links",
                "legacylinks",
                "certificates",
                "caps",
                "settings",
                "login",
                "search",
                "download",
                "ratio",
            ],
            "$",
        )?;
        let search = root
            .get("search")
            .ok_or_else(|| error("$.search", "Missing search"))?;
        allowed(
            search,
            &[
                "paths",
                "path",
                "inputs",
                "headers",
                "rows",
                "fields",
                "method",
                "allowEmptyInputs",
                "keywordsfilters",
                "preprocessingfilters",
                "error",
            ],
            "$.search",
        )?;
        // Unsupported labels are reported by RequestPlan::compile, preserving error order.
        let charset = request::Charset::from_definition(root).unwrap_or_default();
        let keyword_filters = compile_pipeline(
            search.get("keywordsfilters"),
            "$.search.keywordsfilters",
            charset,
        )?;
        let preprocessing_filters = compile_pipeline(
            search.get("preprocessingfilters"),
            "$.search.preprocessingfilters",
            charset,
        )?;
        let errors = compile_errors(search.get("error"))?;
        let rows = search
            .get("rows")
            .ok_or_else(|| error("$.search.rows", "Missing rows"))?;
        allowed(
            rows,
            &[
                "selector",
                "attribute",
                "count",
                "filters",
                "after",
                "dateheaders",
                "multiple",
                "missingAttributeEqualsNoResults",
            ],
            "$.search.rows",
        )?;
        let after = compile_after(rows.get("after"))?;
        let json_shape = selector::JsonRowShape {
            multiple: flag(rows, "multiple")?,
            missing_is_empty: flag(rows, "missingAttributeEqualsNoResults")?,
        };
        let date_headers = rows
            .get("dateheaders")
            .map(|node| compile_date_headers(node, charset))
            .transpose()?;
        let row_filters = rowfilters::RowFilters::compile(root, rows)?;
        if rows
            .get("count")
            .is_some_and(|n| n.get("filters").is_some())
        {
            return Err(error(
                "$.search.rows.count.filters",
                "Count filter pipeline unsupported",
            ));
        }
        let count = rows
            .get("count")
            .map(selector::Selector::compile)
            .transpose()
            .map_err(|e| error("$.search.rows.count", e))?;
        let rowselector = rows
            .string("selector")
            .ok_or_else(|| error("$.search.rows.selector", "Missing selector"))?
            .to_owned();
        let (row_template, row_variants) = Template::compile_selector(&rowselector)
            .map_err(|e| error("$.search.rows.selector", e))?;
        let row_values: Vec<String> = row_template
            .selector_values()
            .into_iter()
            .map(str::to_owned)
            .collect();
        // Substituted values must come from declared text settings, never a
        // checkbox's "True", a multi-select collection or a query value.
        for key in &row_values {
            let declared = root
                .get("settings")
                .and_then(Node::items)
                .unwrap_or_default()
                .iter()
                .any(|s| {
                    s.string("name").map(|n| format!(".Config.{n}")).as_ref() == Some(key)
                        && s.string("type").unwrap_or("text") == "text"
                });
            if !declared {
                return Err(error(
                    "$.search.rows.selector",
                    "Row selector values must come from text settings",
                ));
            }
        }
        let mut fields = Vec::new();
        for (key, node) in search
            .get("fields")
            .and_then(Node::pairs)
            .unwrap_or_default()
        {
            let name = key.text().unwrap_or_default();
            let path = format!("$.search.fields.{name}");
            let mut parts = name.split('|');
            let base = parts.next().unwrap_or_default();
            for modifier in parts {
                if !["optional", "append", "noappend"].contains(&modifier) {
                    return Err(error(
                        &path,
                        format!("Unsupported field modifier {modifier}"),
                    ));
                }
            }
            if base.is_empty() {
                return Err(error(&path, "Empty field"));
            }
            allowed(
                node,
                &[
                    "selector",
                    "text",
                    "attribute",
                    "remove",
                    "case",
                    "filters",
                    "optional",
                    "default",
                ],
                &path,
            )?;
            validate_templates(node, &path)?;
            let s = selector::Selector::compile(node).map_err(|e| error(&path, e))?;
            let mut fs = Vec::new();
            for (i, f) in node
                .get("filters")
                .and_then(Node::items)
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                allowed(f, &["name", "args"], &format!("{path}.filters[{i}]"))?;
                let args = match f.get("args") {
                    Some(n) => {
                        if let Some(v) = n.items() {
                            v.iter()
                                .map(|v| v.text().unwrap_or_default().to_string())
                                .collect()
                        } else {
                            vec![n.text().unwrap_or_default().to_owned()]
                        }
                    }
                    None => Vec::new(),
                };
                if i >= 64 {
                    return Err(error(&path, "Filter count limit exceeded"));
                }
                fs.push(
                    filters::Filter::compile_for(
                        f.string("name").unwrap_or_default(),
                        &args,
                        charset,
                    )
                    .map_err(|e| error(format!("{path}.filters[{i}]"), e))?,
                );
            }
            fields.push(Field {
                name: name.into(),
                selector: s,
                filters: fs,
                optional: node.string("optional") == Some("true")
                    || name.split('|').any(|p| p == "optional")
                    || [
                        "imdb",
                        "imdbid",
                        "tmdbid",
                        "rageid",
                        "tvdbid",
                        "tvmazeid",
                        "traktid",
                        "doubanid",
                        "poster",
                        "genre",
                        "description",
                    ]
                    .contains(&name),
                default: node
                    .string("default")
                    .map(Template::compile)
                    .transpose()
                    .map_err(|e| error(&path, e))?,
            });
        }
        if fields.len() > 128 {
            return Err(error("$.search.fields", "Field count limit exceeded"));
        }
        let mut settings = BTreeMap::new();
        if let Some(link) = root
            .get("links")
            .and_then(Node::items)
            .and_then(|v| v.first())
            .and_then(Node::text)
        {
            settings.insert(".Config.sitelink".into(), Value::String(link.into()));
        }
        for s in root
            .get("settings")
            .and_then(Node::items)
            .unwrap_or_default()
        {
            if let Some(name) = s.string("name") {
                let raw = s.string("default").unwrap_or("");
                let value = if s.string("type") == Some("multi-select") {
                    Value::Array(
                        s.get("defaults")
                            .and_then(Node::items)
                            .unwrap_or_default()
                            .iter()
                            .map(|n| Value::String(n.text().unwrap_or_default().into()))
                            .collect(),
                    )
                } else if s.string("type") == Some("checkbox") {
                    if raw == "true" {
                        Value::String("True".into())
                    } else {
                        Value::Null
                    }
                } else {
                    Value::String(raw.into())
                };
                settings.insert(format!(".Config.{name}"), value);
            }
        }
        let mut cats = Vec::new();
        if let Some(caps) = root.get("caps") {
            let simple = caps
                .get("categories")
                .and_then(Node::pairs)
                .unwrap_or_default();
            let mappings = caps
                .get("categorymappings")
                .and_then(Node::items)
                .unwrap_or_default();
            if simple.len() + mappings.len() > 4096 {
                return Err(error("$.caps", "Category mapping limit exceeded (4096)"));
            }
            let mut bytes = 0usize;
            for text in simple
                .iter()
                .map(|(id, _)| id.text().unwrap_or_default())
                .chain(mappings.iter().flat_map(|m| {
                    [
                        m.string("id").unwrap_or_default(),
                        m.string("desc").unwrap_or_default(),
                    ]
                }))
            {
                bytes += text.len();
                if text.contains('\0') {
                    return Err(error("$.caps", "NUL in category text is unsupported"));
                }
                if text.len() > 4096 || bytes > 1024 * 1024 {
                    return Err(error(
                        "$.caps",
                        "Category text limit exceeded (4096 bytes per value, 1 MiB total)",
                    ));
                }
            }
            for (id, cat) in caps
                .get("categories")
                .and_then(Node::pairs)
                .unwrap_or_default()
            {
                cats.push(Category {
                    id: id.text().unwrap_or_default().into(),
                    desc: None,
                    default: false,
                    cats: vec![category_id(cat.text().unwrap_or_default())?],
                });
            }
            for cat in caps
                .get("categorymappings")
                .and_then(Node::items)
                .unwrap_or_default()
            {
                let id = cat.string("id").unwrap_or_default();
                let mut mapped = vec![category_id(cat.string("cat").unwrap_or_default())?];
                let desc = cat.string("desc").map(str::to_owned);
                if desc.is_some() {
                    mapped.push(
                        categories::custom_id(id)
                            .map_err(|e| error("$.caps.categorymappings", e))?,
                    );
                }
                cats.push(Category {
                    id: id.into(),
                    desc,
                    default: cat.string("default") == Some("true"),
                    cats: mapped,
                });
            }
        }
        let mut blockers = Vec::new();
        for (key, feature) in [
            ("login", "session/login/challenge"),
            ("download", "download-resolution"),
            ("ratio", "session ratio"),
            ("certificates", "transport certificate policy"),
        ] {
            if root.get(key).is_some() {
                blockers.push(Blocker {
                    path: format!("$.{key}"),
                    feature: feature.into(),
                });
            }
        }
        let requests = request::RequestPlan::compile(root).map_err(|e| error("$.search", e))?;
        // A JSON rows selector with values renders through `render_json_selector`; the
        // variants below are still probed as JSON selectors (docs/api-origins.md).
        // Upstream evaluates error blocks only on the HTML branch (CardigannIndexer.cs:1819).
        if !errors.is_empty() && requests.response_types().any(|format| format != "html") {
            return Err(error(
                "$.search.error",
                "Search error blocks are only supported for HTML responses",
            ));
        }
        // Positive values are honoured upstream only on the HTML/XML branches
        // (CardigannIndexer.cs:1834-1852).
        if after > 0 && requests.response_types().any(|format| format == "json") {
            return Err(error(
                AFTER_PATH,
                "Row merging is only supported for HTML responses",
            ));
        }
        for format in requests.response_types() {
            if let Some(count) = &count {
                if format != "json" {
                    return Err(error(
                        "$.search.rows.count",
                        "Count is only supported for JSON",
                    ));
                }
                count
                    .validate_json()
                    .map_err(|e| error("$.search.rows.count", e))?;
            }
            for f in &fields {
                let checked = if format == "json" {
                    f.selector.validate_json()
                } else {
                    f.selector.validate_html()
                };
                checked.map_err(|e| error(format!("$.search.fields.{}", f.name), e))?;
            }
            for variant in &row_variants {
                if variant.trim().is_empty() {
                    return Err(error("$.search.rows.selector", "Empty row selector branch"));
                }
                let probe = cardigann::parse(
                    &format!("selector: {}", serde_json::to_string(variant).unwrap()),
                    cardigann::Limits::default(),
                )
                .map_err(|d| error(d.path, d.message))?;
                let probe = selector::Selector::compile(&probe)
                    .map_err(|e| error("$.search.rows.selector", e))?;
                if format == "json" {
                    probe.validate_json()
                } else {
                    probe.validate_html()
                }
                .map_err(|e| error("$.search.rows.selector", e))?;
            }
        }
        Ok(Self {
            id: root.string("id").unwrap_or_default().into(),
            name: root.string("name").unwrap_or_default().into(),
            requests,
            fields,
            keyword_filters,
            preprocessing_filters,
            errors,
            rows: row_template,
            row_values,
            after,
            date_headers,
            json_shape,
            row_filters,
            count,
            attribute: rows.string("attribute").map(str::to_owned),
            settings,
            categories: cats,
            blockers,
        })
    }
    /// Standard categories represented by this compiled definition.
    pub fn supported_categories(&self) -> Vec<u32> {
        let mut ids: Vec<_> = self
            .categories
            .iter()
            .flat_map(|c| c.cats.iter().copied())
            .filter(|id| *id < 100_000)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
    /// Definition encoding for search requests and raw response decoding.
    pub fn charset(&self) -> request::Charset {
        self.requests.charset()
    }
    pub fn blockers(&self) -> &[Blocker] {
        &self.blockers
    }
    pub fn context(
        &self,
        settings: &BTreeMap<String, Value>,
        query: &SearchQuery,
        now: DateTime<Utc>,
    ) -> Context {
        use chrono::Datelike;
        let mut values = self.settings.clone();
        values.insert(
            ".Today.Year".into(),
            Value::String((now.year() - i32::from(now.month() == 1)).to_string()),
        );
        for (k, v) in settings {
            let key = format!(".Config.{k}");
            let value = match v {
                // Row selector values reject non-text later instead of coercing.
                _ if self.row_values.contains(&key) => v.clone(),
                Value::Bool(true) => Value::String("True".into()),
                Value::Bool(false) => Value::Null,
                _ => v.clone(),
            };
            values.insert(key, value);
        }
        values.insert(".Keywords".into(), Value::String(query.keywords()));
        for (name, key) in [
            ("Type", "t"),
            ("Q", "q"),
            ("Season", "season"),
            ("Ep", "ep"),
            ("Limit", "limit"),
            ("Offset", "offset"),
            ("Artist", "artist"),
            ("Album", "album"),
            ("Author", "author"),
            ("Title", "title"),
            ("Publisher", "publisher"),
        ] {
            values.insert(
                format!(".Query.{name}"),
                Value::String(query.get(key).into()),
            );
        }
        values.insert(
            ".Query.IMDBIDShort".into(),
            if query.get("imdbid").is_empty() {
                Value::Null
            } else {
                Value::String(query.get("imdbid").trim_start_matches("tt").into())
            },
        );
        values.insert(".Query.Keywords".into(), Value::String(query.keywords()));
        values.insert(
            ".Query.Episode".into(),
            Value::String(
                query
                    .keywords()
                    .strip_prefix(query.get("q"))
                    .unwrap_or("")
                    .trim()
                    .into(),
            ),
        );
        for (name, key) in [
            ("Year", "year"),
            ("Genre", "genre"),
            ("Label", "label"),
            ("Track", "track"),
        ] {
            values.insert(
                format!(".Query.{name}"),
                Value::String(query.get(key).into()),
            );
        }
        // Pinned CardigannIndexer.cs:1459–1466 stores absent IDs as null,
        // the same value as .False. An empty string is not equal to .False:
        // NexusPHP's keyword fallback would otherwise omit the search term.
        for (name, key) in [
            ("IMDBID", "imdbid"),
            ("TMDBID", "tmdbid"),
            ("TVDBID", "tvdbid"),
            ("TVRageID", "rid"),
            ("TVMazeID", "tvmazeid"),
            ("TraktID", "traktid"),
            ("DoubanID", "doubanid"),
        ] {
            let value = query.get(key);
            values.insert(
                format!(".Query.{name}"),
                if value.is_empty() {
                    Value::Null
                } else {
                    Value::String(value.into())
                },
            );
        }
        for name in ["Series", "Movie", "APIKey", "Extended"] {
            values.insert(format!(".Query.{name}"), Value::Null);
        }
        for (name, t) in [
            ("IsSearch", "search"),
            ("IsTVSearch", "tvsearch"),
            ("IsMovieSearch", "movie"),
            ("IsMusicSearch", "music"),
            ("IsBookSearch", "book"),
        ] {
            values.insert(
                format!(".Query.{name}"),
                if query.get("t") == t {
                    Value::String("True".into())
                } else {
                    Value::Null
                },
            );
        }
        for (name, key) in [
            ("IsImdbQuery", "imdbid"),
            ("IsTmdbQuery", "tmdbid"),
            ("IsTvdbQuery", "tvdbid"),
            ("IsTVRageQuery", "rid"),
            ("IsTvmazeQuery", "tvmazeid"),
            ("IsTraktQuery", "traktid"),
            ("IsDoubanQuery", "doubanid"),
            ("IsGenreQuery", "genre"),
        ] {
            values.insert(
                format!(".Query.{name}"),
                if !query.get(key).is_empty() {
                    Value::String("True".into())
                } else {
                    Value::Null
                },
            );
        }
        let idsearch = [
            "imdbid", "tmdbid", "tvdbid", "rid", "tvmazeid", "traktid", "doubanid",
        ]
        .iter()
        .any(|k| !query.get(k).is_empty());
        values.insert(
            ".Query.IsIdSearch".into(),
            if idsearch {
                Value::String("True".into())
            } else {
                Value::Null
            },
        );
        values.insert(
            ".Query.IsRssSearch".into(),
            if query.keywords().is_empty() && !idsearch {
                Value::String("True".into())
            } else {
                Value::Null
            },
        );
        let requested: std::collections::BTreeSet<u32> = query
            .get("cat")
            .split(',')
            .filter_map(|v| v.parse().ok())
            .collect();
        let mut tracker = Vec::new();
        for c in &self.categories {
            if c.cats.iter().any(|id| {
                requested.contains(id)
                    || categories::parent(*id).is_some_and(|parent| requested.contains(&parent))
            }) && !tracker.contains(&Value::String(c.id.clone()))
            {
                tracker.push(Value::String(c.id.clone()));
            }
        }
        if tracker.is_empty() {
            tracker = self
                .categories
                .iter()
                .filter(|c| c.default)
                .map(|c| Value::String(c.id.clone()))
                .collect();
        }
        values.insert(".Categories".into(), Value::Array(tracker));
        Context { values, now }
    }
    /// Apply keyword filters once from the original query, retaining immutable caller context.
    /// Request construction and extraction also call this method; repeated preparation is safe.
    pub fn prepare_context(&self, ctx: &Context) -> Result<Context, ExecutionError> {
        if self.keyword_filters.is_empty() {
            return Ok(ctx.clone());
        }
        let mut prepared = ctx.clone();
        let original = ctx
            .values
            .get(".Query.Keywords")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                error(
                    "$.search.keywordsfilters",
                    "Missing original query keywords",
                )
            })?;
        // Upstream assigns .Keywords only after the complete pipeline has run.
        prepared.values.remove(".Keywords");
        let mut budget = Budget {
            work: 32 * 1024 * 1024,
            operations: 20000,
            output: 8 * 1024 * 1024,
        };
        let transformed = apply_pipeline(
            &self.keyword_filters,
            original,
            &prepared,
            &mut budget,
            "$.search.keywordsfilters",
        )?;
        prepared
            .values
            .insert(".Keywords".into(), Value::String(transformed));
        Ok(prepared)
    }
    pub fn build_requests(
        &self,
        ctx: &Context,
    ) -> Result<Vec<request::BuiltRequest>, ExecutionError> {
        let ctx = self.prepare_context(ctx)?;
        // Configured row selector values are rejected before any request exists.
        self.check_row_selector(&ctx)?;
        self.requests.build(&ctx).map_err(|e| error("$.search", e))
    }
    /// Check resolved settings once, as a source is configured, so invalid row selector
    /// values never wait for a search. Requests and extraction check them again.
    pub fn check_settings(&self, settings: &BTreeMap<String, Value>) -> Result<(), ExecutionError> {
        let ctx = self.context(settings, &SearchQuery::default(), Utc::now());
        self.check_row_selector(&ctx)
    }
    /// Render the row selector as every response type of this plan will.
    fn check_row_selector(&self, ctx: &Context) -> Result<(), ExecutionError> {
        if self
            .requests
            .response_types()
            .any(|format| format == "json")
        {
            self.rows
                .render_json_selector(ctx)
                .map_err(|e| error("$.search.rows.selector", e))?;
        }
        if self
            .requests
            .response_types()
            .any(|format| format != "json")
        {
            self.row_selector(ctx)?;
        }
        Ok(())
    }
    fn row_selector(&self, ctx: &Context) -> Result<String, ExecutionError> {
        self.rows
            .render_selector(ctx)
            .map_err(|e| error("$.search.rows.selector", e))
    }
    /// Extraction without transport metadata. Requests carrying the empty no-results policy
    /// fail closed because HTTP 200 and the absence of redirects cannot be established.
    pub fn extract(
        &self,
        request: &request::BuiltRequest,
        body: &str,
        ctx: &Context,
    ) -> Result<Vec<Release>, ExecutionError> {
        if body.len() > 8 * 1024 * 1024 {
            return Err(error("$.response", "Response byte budget exceeded"));
        }
        if request.empty_no_results() {
            return Err(error(
                "$.response",
                "Response metadata is required for the no-results policy",
            ));
        }
        self.extract_body(request, body, ctx, false)
    }
    /// Extraction with transport metadata. Requests without the empty no-results policy are
    /// handled exactly as by `extract`; the metadata is not consulted for them.
    pub fn extract_response(
        &self,
        request: &request::BuiltRequest,
        response: &ResponseMeta,
        body: &str,
        ctx: &Context,
    ) -> Result<Vec<Release>, ExecutionError> {
        if !request.empty_no_results() {
            return self.extract(request, body, ctx);
        }
        if body.len() > 8 * 1024 * 1024 {
            return Err(error("$.response", "Response byte budget exceeded"));
        }
        if request.response_type != "json" || !self.requests.has_empty_no_results() {
            return Err(error(
                "$.response",
                "No-results policy does not match this plan",
            ));
        }
        // CardigannIndexer.cs:1631-1645 rejects every non-200 JSON response and re-authenticates
        // after redirects (WebResult.IsRedirect includes Refresh); neither may become empty.
        if response.status != 200 {
            return Err(error("$.response", "No-results policy requires HTTP 200"));
        }
        if response.redirected || response.refresh {
            return Err(error(
                "$.response",
                "No-results policy rejects redirected responses",
            ));
        }
        // Only a zero-byte body equals string.Empty; a decoded BOM or whitespace does not.
        if response.body_bytes == 0 && body.is_empty() {
            return Ok(Vec::new());
        }
        self.extract_body(request, body, ctx, false)
    }
    /// Explicit public-tracker runtime policy: retain valid HTML rows when a row has
    /// a field error, matching Jackett's per-row catch. Resource/document failures
    /// still abort. If every selected row fails, the result is empty, as in Jackett.
    pub fn extract_response_skipping_invalid_html_rows(
        &self,
        request: &request::BuiltRequest,
        response: &ResponseMeta,
        body: &str,
        ctx: &Context,
    ) -> Result<Vec<Release>, ExecutionError> {
        if request.response_type != "html" || request.empty_no_results() {
            return self.extract_response(request, response, body, ctx);
        }
        if body.len() > 8 * 1024 * 1024 {
            return Err(error("$.response", "Response byte budget exceeded"));
        }
        self.extract_body(request, body, ctx, true)
    }
    fn extract_body(
        &self,
        request: &request::BuiltRequest,
        body: &str,
        ctx: &Context,
        skip_invalid_rows: bool,
    ) -> Result<Vec<Release>, ExecutionError> {
        let prepared = self.prepare_context(ctx)?;
        let ctx = &prepared;
        let mut releases = Vec::new();
        let mut budget = Budget {
            work: 32 * 1024 * 1024,
            operations: 20000,
            output: 8 * 1024 * 1024,
        };
        if request.response_type == "json" {
            if self.after > 0 {
                return Err(error(
                    AFTER_PATH,
                    "Row merging is only supported for HTML responses",
                ));
            }
            let json: Value = serde_json::from_str(body)
                .map_err(|_| error("$.response", "Invalid or over-deep JSON"))?;
            if let Some(count) = &self.count {
                if let Ok(Some(n)) =
                    count.extract_json(&json, &|v| Template::compile(v)?.render(ctx))
                {
                    if n.parse::<i64>().is_ok_and(|n| n < 1) {
                        return Ok(Vec::new());
                    }
                }
            }
            let rows = self
                .rows
                .render_json_selector(ctx)
                .map_err(|e| error("$.search.rows.selector", e))?;
            for (parent, row) in selector::json_row_pairs(
                &json,
                &rows,
                self.attribute.as_deref(),
                self.json_shape,
                1000,
            )
            .map_err(|e| error("$.search.rows", e))?
            {
                releases.push(self.release(ctx, &request.url, &mut budget, |s, c| {
                    let target = if s.json_parent() { parent } else { row };
                    s.extract_json(target, &|v| Template::compile(v)?.render(c))
                })?);
            }
        } else if matches!(request.response_type.as_str(), "html" | "xml") {
            let parse = |source: &str| {
                if request.response_type == "xml" {
                    xml::parse_document(source).map_err(|e| error("$.response", e))
                } else {
                    Ok(css::parse_document(source))
                }
            };
            // Upstream checks error blocks on the raw document before preprocessing
            // (CardigannIndexer.cs:1816-1826); without filters that DOM is reused.
            let raw = if request.response_type == "xml" {
                Some(parse(body)?)
            } else if self.errors.is_empty() {
                None
            } else {
                let html = css::parse_document(body);
                self.check_errors(&html)?;
                Some(html)
            };
            let mut html = match raw {
                Some(html) if self.preprocessing_filters.is_empty() => html,
                _ => parse(&apply_pipeline(
                    &self.preprocessing_filters,
                    body,
                    ctx,
                    &mut budget,
                    "$.search.preprocessingfilters",
                )?)?,
            };
            let mut visits = 20_000_000;
            let rows = self.row_selector(ctx)?;
            let mut slots = selector::html_row_slots(&html, &rows, 1000).map_err(|e| {
                if css::is_execution_failure(&e) {
                    error("$.response", e)
                } else {
                    error("$.search.rows", e)
                }
            })?;
            // Every group merges before the first field reads or removes anything.
            if self.after > 0 {
                slots = selector::merge_after_rows(&mut html, slots, self.after, &mut visits)
                    .map_err(|e| {
                        if e.contains("aggregate visit budget") || e.starts_with("HTML document") {
                            error("$.response", e)
                        } else {
                            error(AFTER_PATH, e)
                        }
                    })?;
            }
            for row in slots {
                let release = self
                    .release(ctx, &request.url, &mut budget, |s, c| {
                        s.extract_html_mut(&mut html, row, &mut visits, &|v| {
                            Template::compile(v)?.render(c)
                        })
                    })
                    .and_then(|mut release| {
                        self.date_header(
                            &mut release,
                            &mut html,
                            row,
                            &mut visits,
                            ctx,
                            &request.url,
                            &mut budget,
                        )?;
                        Ok(release)
                    });
                match release {
                    Ok(release) => releases.push(release),
                    Err(e)
                        if skip_invalid_rows
                            && (e.path.starts_with("$.search.fields.")
                                || e.path.starts_with(DATE_HEADERS_PATH)) => {}
                    Err(e) => return Err(e),
                }
            }
        } else {
            return Err(error("$.search.response.type", "Unsupported response type"));
        }
        if let Some(matcher) = self.row_filters.prepare(ctx)? {
            let mut matched = Vec::new();
            for release in releases {
                if matcher.matches(&release.title, &mut budget)? {
                    matched.push(release);
                }
            }
            releases = matched;
        }
        Ok(releases)
    }
    /// Upstream `Rows.Dateheaders` (CardigannIndexer.cs:1910-1946). A row without a
    /// publish date takes it from the nearest earlier element whose descendants match the
    /// block: previous siblings first, then the previous sibling of the row's parent, and so
    /// on. A selector miss or filter failure moves to the next element. Without a match a
    /// required block fails the row; the header value is parsed like a `date` field.
    #[allow(clippy::too_many_arguments)]
    fn date_header(
        &self,
        release: &mut Release,
        html: &mut scraper::Html,
        row: usize,
        visits: &mut usize,
        ctx: &Context,
        url: &str,
        budget: &mut Budget,
    ) -> Result<(), ExecutionError> {
        let Some(headers) = &self.date_headers else {
            return Ok(());
        };
        if release.published.is_some() {
            return Ok(());
        }
        let response_failure = |e: String| {
            if e.contains("aggregate visit budget") || css::is_execution_failure(&e) {
                Some(error("$.response", e))
            } else {
                None
            }
        };
        let mut candidate = selector::previous_header_slot(html, row, visits)
            .map_err(|e| error("$.response", e))?;
        let mut value = None;
        while let Some(slot) = candidate {
            budget.operation()?;
            let selected = match headers
                .selector
                .extract_html_mut(html, slot, visits, &|v| Template::compile(v)?.render(ctx))
            {
                Ok(selected) => selected,
                Err(e) => match response_failure(e) {
                    Some(e) => return Err(e),
                    None => None,
                },
            };
            if let Some(mut v) = selected {
                budget.consume(v.len())?;
                let mut filtered = true;
                for filter in &headers.filters {
                    budget.operation()?;
                    budget.consume(v.len())?;
                    match filter.apply(&v, ctx) {
                        Ok(next) => v = next,
                        Err(_) => {
                            filtered = false;
                            break;
                        }
                    }
                }
                if filtered {
                    value = Some(v);
                    break;
                }
            }
            candidate = selector::previous_header_slot(html, slot, visits)
                .map_err(|e| error("$.response", e))?;
        }
        match value {
            Some(v) => self
                .normalize(release, "date", "date", &v, url, ctx)
                .map(|_| ())
                .map_err(|e| error(DATE_HEADERS_PATH, e)),
            None if headers.optional => Ok(()),
            None => Err(error(DATE_HEADERS_PATH, "No date header row found")),
        }
    }
    /// First matching error block fails the response. Neither the page, the selector nor
    /// the definition's message is reported: only the block's source path. Documents
    /// whose tree is known to differ from upstream (declarative shadow root templates,
    /// select elements) fail closed before any block is matched, even
    /// where upstream would find no error; that is bounded compatibility, not parity.
    fn check_errors(&self, html: &scraper::Html) -> Result<(), ExecutionError> {
        selector::check_document(html).map_err(|e| error("$.response", e))?;
        css::check_error_document(html).map_err(|e| error("$.response", e))?;
        for (i, css) in self.errors.iter().enumerate() {
            if selector::document_matches(html, css).map_err(|e| error("$.response", e))? {
                return Err(error(
                    format!("$.search.error[{i}]"),
                    "Source reported a search error",
                ));
            }
        }
        Ok(())
    }
    fn release(
        &self,
        ctx: &Context,
        url: &str,
        budget: &mut Budget,
        mut extract: impl FnMut(&selector::Selector, &Context) -> Result<Option<String>, String>,
    ) -> Result<Release, ExecutionError> {
        let mut ctx = ctx.clone();
        ctx.values.retain(|k, _| !k.starts_with(".Result."));
        let mut r = Release {
            indexer_id: self.id.clone(),
            indexer: self.name.clone(),
            ..Release::default()
        };
        for f in &self.fields {
            budget.operation()?;
            let path = format!("$.search.fields.{}", f.name);
            let base = f.name.split('|').next().unwrap_or_default();
            let evaluated = (|| -> Result<String, ExecutionError> {
                let selected = extract(&f.selector, &ctx).map_err(|e| {
                    // Response-wide failures must not become optional-field misses.
                    if e.contains("aggregate visit budget") || css::is_execution_failure(&e) {
                        error("$.response", e)
                    } else {
                        error(&path, e)
                    }
                })?;
                let mut value = selected;
                if let Some(v) = &mut value {
                    budget.consume(v.len())?;
                    for filter in &f.filters {
                        budget.operation()?;
                        budget.consume(v.len())?;
                        *v = filter.apply(v, &ctx).map_err(|e| error(&path, e))?;
                    }
                }
                let value = if f.optional && value.as_deref().is_none_or(|v| v.trim().is_empty()) {
                    f.default
                        .as_ref()
                        .map(|t| t.render(&ctx))
                        .transpose()
                        .map_err(|e| error(&path, e))?
                        .filter(|v| !v.trim().is_empty())
                        .ok_or_else(|| error(&path, "Optional field absent"))?
                } else {
                    value.ok_or_else(|| error(&path, "Required field did not match"))?
                };
                if matches!(base, "category" | "categorydesc") {
                    for category in &self.categories {
                        budget.operation()?;
                        budget.consume(
                            value.len()
                                + if base == "category" {
                                    category.id.len()
                                } else {
                                    category.desc.as_deref().unwrap_or_default().len()
                                },
                        )?;
                    }
                }
                self.normalize(&mut r, base, &f.name, &value, url, &ctx)
                    .map_err(|e| error(&path, e))
            })();
            let value = match evaluated {
                Ok(v) => v,
                Err(e) if f.optional && e.path != "$.response" => {
                    ctx.values.insert(format!(".Result.{base}"), Value::Null);
                    continue;
                }
                Err(e) => return Err(e),
            };
            if release_bytes(&r) > 1024 * 1024 {
                return Err(error(&path, "Release output limit exceeded"));
            }
            ctx.values
                .insert(format!(".Result.{base}"), Value::String(value));
        }
        if r.title.is_empty() {
            return Err(error("$.search.fields.title", "Missing title"));
        }
        r.guid = if !r.link.is_empty() {
            r.link.clone()
        } else {
            r.attributes
                .get("magnet")
                .cloned()
                .or_else(|| r.details.clone())
                .unwrap_or_default()
        };
        if let Some(m) = r.attributes.get("magnet") {
            let u = url::Url::parse(m)
                .map_err(|_| error("$.search.fields.magnet", "Invalid magnet"))?;
            if let Some((_, xt)) = u
                .query_pairs()
                .find(|(k, v)| k == "xt" && v.starts_with("urn:btih:"))
            {
                r.attributes
                    .entry("infohash".into())
                    .or_insert_with(|| xt.trim_start_matches("urn:btih:").into());
            }
        }
        if let (Some(size), Some(seeders)) = (r.size, r.seeders) {
            r.attributes.insert(
                "gain".into(),
                (seeders as f64 * size as f64 / 1073741824.0).to_string(),
            );
        }
        let bytes = release_bytes(&r);
        budget.output = budget
            .output
            .checked_sub(bytes)
            .ok_or_else(|| error("$.response", "Aggregate release output budget exceeded"))?;
        Ok(r)
    }
    fn normalize(
        &self,
        r: &mut Release,
        key: &str,
        full: &str,
        v: &str,
        url: &str,
        ctx: &Context,
    ) -> Result<String, String> {
        let mut value = v.to_owned();
        let append = full.split('|').any(|m| m == "append");
        match key {
            "title" => {
                if append {
                    r.title.push_str(v)
                } else {
                    r.title = v.into()
                }
                value = r.title.clone();
            }
            "details" => {
                value = resolve(url, v)?;
                r.details = Some(value.clone());
            }
            "download" => {
                if v.is_empty() {
                    r.link.clear();
                    return Ok(String::new());
                }
                value = resolve(url, v)?;
                if value.starts_with("magnet:") {
                    r.attributes.insert("magnet".into(), value.clone());
                } else {
                    r.link = value.clone();
                }
            }
            "magnet" => {
                value = resolve(url, v)?;
                if !value.starts_with("magnet:") {
                    return Err("Expected magnet URI".into());
                }
                r.attributes.insert(key.into(), value.clone());
            }
            "size" => {
                let size = parse_size(v)?;
                r.size = Some(size);
                value = size.to_string();
            }
            "seeders" | "leechers" => {
                let n = v
                    .replace(',', "")
                    .parse::<u64>()
                    .map_err(|_| "Invalid peer number")?;
                let n = if n < 5_000_000 { n } else { 0 };
                if key == "seeders" {
                    r.seeders = Some(n);
                }
                r.peers = Some(
                    r.peers
                        .unwrap_or_default()
                        .checked_add(n)
                        .ok_or("Peer overflow")?,
                );
                value = n.to_string();
            }
            "category" | "categorydesc" => {
                let mut mapped = Vec::new();
                for c in &self.categories {
                    let candidate = if key == "category" {
                        Some(c.id.as_str())
                    } else {
                        c.desc.as_deref()
                    };
                    if candidate
                        .map(|candidate| categories::matches(candidate, v))
                        .transpose()?
                        .unwrap_or(false)
                    {
                        mapped.extend_from_slice(&c.cats);
                    }
                }
                if !mapped.is_empty() {
                    if full.split('|').any(|m| m == "noappend") {
                        r.categories = mapped;
                    } else {
                        let mut union = Vec::new();
                        let mut seen = std::collections::BTreeSet::new();
                        for id in r.categories.iter().copied().chain(mapped) {
                            if seen.insert(id) {
                                union.push(id);
                            }
                        }
                        r.categories = union;
                    }
                }
            }
            "date" => {
                let date = filters::parse_fuzzy_date(v, ctx)?;
                r.published = Some(date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
                value = date.format("%a, %d %b %Y %H:%M:%S +0000").to_string();
            }
            "poster" if !v.is_empty() => {
                value = resolve(url, v)?;
                r.attributes.insert(key.into(), value.clone());
            }
            "imdb" | "imdbid" | "tmdbid" | "rageid" | "tvdbid" | "tvmazeid" | "traktid"
            | "doubanid" => {
                value = v
                    .chars()
                    .skip_while(|c| !c.is_ascii_digit())
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<u64>()
                    .unwrap_or_default()
                    .to_string();
                r.attributes.insert(
                    if key == "imdb" { "imdbid" } else { key }.into(),
                    value.clone(),
                );
            }
            "files" | "grabs" | "minimumseedtime" | "year" => {
                value = v
                    .parse::<u64>()
                    .map_err(|_| "Invalid integer field")?
                    .to_string();
                r.attributes.insert(key.into(), value.clone());
            }
            "downloadvolumefactor" | "uploadvolumefactor" | "minimumratio" => {
                let n = v.parse::<f64>().map_err(|_| "Invalid numeric field")?;
                if !n.is_finite() || n < 0.0 {
                    return Err("Invalid nonfinite/negative factor".into());
                }
                value = n.to_string();
                r.attributes.insert(key.into(), value.clone());
            }
            "genre" => {
                let mut genres: Vec<String> = r
                    .attributes
                    .get("genre")
                    .map(|s| s.split(',').map(str::to_owned).collect())
                    .unwrap_or_default();
                for g in v
                    .split([',', ' ', '/', ')', '(', '.', ';', '[', ']', '"', '|', ':'])
                    .filter(|g| !g.is_empty())
                {
                    let g = g.replace('_', " ");
                    if !genres.contains(&g) {
                        genres.push(g);
                    }
                }
                value = genres.join(",");
                r.attributes.insert("genre".into(), value.clone());
            }
            "description" | "author" | "booktitle" | "publisher" | "artist" | "album" | "label"
            | "track" | "infohash" => {
                if append {
                    value = format!(
                        "{}{v}",
                        r.attributes
                            .get(key)
                            .map(String::as_str)
                            .unwrap_or_default()
                    );
                }
                r.attributes.insert(key.into(), value.clone());
            }
            _ => {} // upstream retains arbitrary intermediates only in request-local .Result
        }
        Ok(value)
    }
}
struct Budget {
    work: usize,
    operations: usize,
    output: usize,
}
impl Budget {
    fn consume(&mut self, n: usize) -> Result<(), ExecutionError> {
        self.work = self
            .work
            .checked_sub(n)
            .ok_or_else(|| error("$.response", "Extraction work byte budget exceeded"))?;
        Ok(())
    }
    fn operation(&mut self) -> Result<(), ExecutionError> {
        self.operations = self
            .operations
            .checked_sub(1)
            .ok_or_else(|| error("$.response", "Extraction operation budget exceeded"))?;
        Ok(())
    }
}
fn release_bytes(r: &Release) -> usize {
    r.title.len()
        + r.guid.len()
        + r.link.len()
        + r.details.as_ref().map_or(0, String::len)
        + r.attributes
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
}
/// Upstream `MatchSelector` with an attribute (`CardigannIndexer.cs:2048-2080`): the
/// first element matching `selector_text` in document order (AngleSharp
/// `QuerySelector`) and that attribute's value, within the search document's node,
/// depth and visit bounds. A matched element lacking the attribute fails, as upstream.
pub fn query_selector_attribute(
    document: &str,
    selector_text: &str,
    attribute: &str,
) -> Result<Option<String>, String> {
    let html = css::parse_document(document);
    selector::check_document(&html)?;
    let css = css::Css::parse(selector_text)?;
    selector::document_first(&html, &css)?
        .map(|element| {
            element
                .value()
                .attr(attribute)
                .map(str::to_owned)
                .ok_or_else(|| "Matched element lacks the selected attribute".to_owned())
        })
        .transpose()
}
/// Login presence checks share the search parser, inert-template handling and bounds.
/// Positive local selectors can prove login outside ordinary HTML select forms.
/// Structural/text selectors retain the stricter whole-document parser guards.
pub fn query_selector_exists(document: &str, selector_text: &str) -> Result<bool, String> {
    if document.len() > 8 * 1024 * 1024 {
        return Err("HTML document exceeds byte budget".into());
    }
    let html = css::parse_presence_document(document)?;
    selector::check_document(&html)?;
    let css = selector::parse_css(selector_text)?;
    if css::check_presence_document(&html, &css)? {
        selector::document_local_presence(&html, &css)
    } else {
        selector::document_matches(&html, &css)
    }
}
/// A captured positive empty-result marker: first matching, non-inert local
/// element, containing only text/comments. Nested markup cannot prove emptiness.
/// Uses the same parser, whole-document bounds and select/shadow guards as login.
pub fn query_leaf_text_equals(
    document: &str,
    selector_text: &str,
    expected: &str,
) -> Result<bool, String> {
    if document.len() > 8 * 1024 * 1024 || expected.len() > 4096 {
        return Err("HTML marker byte budget exceeded".into());
    }
    let html = css::parse_presence_document(document)?;
    selector::check_document(&html)?;
    let css = selector::parse_css(selector_text)?;
    if !css.is_local_presence() {
        return Err("HTML marker requires a positive local selector".into());
    }
    css::check_presence_document(&html, &css)?;
    let Some(element) = selector::document_first_filtered(&html, &css, true)? else {
        return Ok(false);
    };
    let mut text = String::new();
    for child in element.children() {
        match child.value() {
            scraper::Node::Text(value) => {
                if text.len().saturating_add(value.text.len()) > 4096 {
                    return Err("HTML marker text budget exceeded".into());
                }
                text.push_str(&value.text);
            }
            scraper::Node::Comment(_) => {}
            _ => return Ok(false),
        }
    }
    Ok(text.trim() == expected)
}
fn compile_pipeline(
    node: Option<&Node>,
    path: &str,
    charset: request::Charset,
) -> Result<Vec<filters::Filter>, ExecutionError> {
    let Some(node) = node else {
        return Ok(Vec::new());
    };
    let items = node
        .items()
        .ok_or_else(|| error(path, "Filter pipeline must be a sequence"))?;
    if items.len() > 64 {
        return Err(error(path, "Filter count limit exceeded"));
    }
    items
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let path = format!("{path}[{i}]");
            allowed(f, &["name", "args"], &path)?;
            let args = match f.get("args") {
                None => Vec::new(),
                Some(n) => match n.items() {
                    Some(items) => items
                        .iter()
                        .map(|v| {
                            v.text()
                                .map(str::to_owned)
                                .ok_or_else(|| error(&path, "Filter arguments must be scalar"))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    None => vec![n
                        .text()
                        .ok_or_else(|| error(&path, "Filter argument must be scalar"))?
                        .to_owned()],
                },
            };
            filters::Filter::compile_for(f.string("name").unwrap_or_default(), &args, charset)
                .map_err(|e| error(path, e))
        })
        .collect()
}
/// `search.error` (IndexerDefinition.cs:104-109,148): ordered `selector` blocks with an
/// optional literal `message.text`. `path` is unused by upstream search and rejected.
/// The message only replaces upstream's exception text, which is never displayed here,
/// so it is validated and discarded.
fn compile_errors(node: Option<&Node>) -> Result<Vec<css::Css>, ExecutionError> {
    const PATH: &str = "$.search.error";
    let Some(node) = node else {
        return Ok(Vec::new());
    };
    let items = node
        .items()
        .ok_or_else(|| error(PATH, "Error blocks must be a sequence"))?;
    if items.is_empty() {
        return Err(error(PATH, "Empty error block sequence"));
    }
    if items.len() > 8 {
        return Err(error(PATH, "Error block count limit exceeded (8)"));
    }
    let mut compiled = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("{PATH}[{i}]");
        if item.pairs().is_none() {
            return Err(error(&path, "Error block must be a mapping"));
        }
        allowed(item, &["selector", "message"], &path)?;
        let selector_path = format!("{path}.selector");
        let source = item
            .get("selector")
            .ok_or_else(|| error(&selector_path, "Missing selector"))?
            .text()
            .ok_or_else(|| error(&selector_path, "Selector must be a scalar"))?;
        if source.contains("{{") {
            return Err(error(
                &selector_path,
                "Error selector templates unsupported",
            ));
        }
        if source.trim().is_empty() {
            return Err(error(&selector_path, "Empty CSS selector"));
        }
        compiled.push(selector::parse_css(source).map_err(|e| error(&selector_path, e))?);
        if let Some(message) = item.get("message") {
            let path = format!("{path}.message");
            if message.pairs().is_none() {
                return Err(error(&path, "Error message must be a mapping"));
            }
            allowed(message, &["text"], &path)?;
            let path = format!("{path}.text");
            let text = message
                .get("text")
                .ok_or_else(|| error(&path, "Missing literal text"))?
                .text()
                .ok_or_else(|| error(&path, "Error message text must be a scalar"))?;
            if text.contains("{{") {
                return Err(error(&path, "Error message templates unsupported"));
            }
            if text.len() > 4096 {
                return Err(error(&path, "Error message length limit exceeded (4096)"));
            }
        }
    }
    Ok(compiled)
}
const AFTER_PATH: &str = "$.search.rows.after";
const DATE_HEADERS_PATH: &str = "$.search.rows.dateheaders";
/// A boolean row property; YAML `true`/`false` only.
fn flag(rows: &Node, key: &str) -> Result<bool, ExecutionError> {
    match rows.get(key).map(|n| n.text()) {
        None => Ok(false),
        Some(Some("true")) => Ok(true),
        Some(Some("false")) => Ok(false),
        Some(_) => Err(error(
            format!("$.search.rows.{key}"),
            "Expected true or false",
        )),
    }
}
fn compile_date_headers(
    node: &Node,
    charset: request::Charset,
) -> Result<DateHeaders, ExecutionError> {
    allowed(
        node,
        &["selector", "attribute", "filters", "optional"],
        DATE_HEADERS_PATH,
    )?;
    validate_templates(node, DATE_HEADERS_PATH)?;
    Ok(DateHeaders {
        selector: selector::Selector::compile(node).map_err(|e| error(DATE_HEADERS_PATH, e))?,
        filters: compile_pipeline(
            node.get("filters"),
            &format!("{DATE_HEADERS_PATH}.filters"),
            charset,
        )?,
        optional: node.string("optional") == Some("true"),
    })
}
/// `rowsBlock.After` is a YamlDotNet `int`. Only the unambiguous plain decimal subset is
/// accepted; quoted, signed-plus, leading-zero, fractional and overflowing forms are
/// rejected rather than guessed. Non-positive values disable merging, as upstream.
fn compile_after(node: Option<&Node>) -> Result<usize, ExecutionError> {
    let Some(node) = node else {
        return Ok(0);
    };
    let cardigann::Value::Scalar(cardigann::Scalar {
        text,
        style: cardigann::ScalarStyle::Plain,
    }) = &node.value
    else {
        return Err(error(AFTER_PATH, "After must be a plain integer scalar"));
    };
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return Err(error(AFTER_PATH, "After must be a decimal integer"));
    }
    let value: i32 = text
        .parse()
        .map_err(|_| error(AFTER_PATH, "After integer out of range"))?;
    Ok(usize::try_from(value).unwrap_or(0))
}
fn apply_pipeline(
    filters: &[filters::Filter],
    input: &str,
    ctx: &Context,
    budget: &mut Budget,
    path: &str,
) -> Result<String, ExecutionError> {
    // Empty pipelines preserve the response admission limit; active filters impose their
    // independent 1 MiB bound before any operation can allocate expanded output.
    let mut value = input.to_owned();
    for (i, filter) in filters.iter().enumerate() {
        let path = format!("{path}[{i}]");
        budget.consume(value.len())?;
        budget.operation()?;
        value = filter.apply(&value, ctx).map_err(|e| error(&path, e))?;
        budget.consume(value.len())?;
    }
    Ok(value)
}
fn allowed(node: &Node, keys: &[&str], path: &str) -> Result<(), ExecutionError> {
    let mut seen = std::collections::BTreeSet::new();
    for (k, _) in node.pairs().unwrap_or_default() {
        let key = k.text().unwrap_or_default();
        if !seen.insert(key) {
            return Err(error(
                format!("{path}.{key}"),
                "Ambiguous repeated structural property",
            ));
        }
        if !keys.contains(&key) {
            return Err(error(
                format!("{path}.{key}"),
                "Unsupported execution semantic",
            ));
        }
    }
    Ok(())
}
fn category_id(v: &str) -> Result<u32, ExecutionError> {
    categories::category(v).ok_or_else(|| error("$.caps", format!("Unsupported category name {v}")))
}
fn resolve(base: &str, v: &str) -> Result<String, String> {
    let u = url::Url::parse(base)
        .and_then(|b| b.join(v))
        .map_err(|_| "Invalid result URL")?;
    if !["http", "https", "magnet"].contains(&u.scheme()) {
        return Err("Unsupported result URL scheme".into());
    }
    Ok(u.into())
}
// Jackett ParseUtil.GetBytes collects digits/separators and letters independently.
// In particular, 1337x size cells include a numeric badge after the unit. Keep that
// numeric normalization instead of requiring the entire suffix to be a unit.
fn parse_size(v: &str) -> Result<u64, String> {
    if v.chars()
        .any(|c| !c.is_ascii() && (c.is_numeric() || c.is_alphabetic()))
    {
        return Err("Unsupported size characters".into());
    }
    let mut number: String = v
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .map(|c| if c == ',' { '.' } else { c })
        .collect();
    if let Some(last) = number.rfind('.') {
        number = number[..last].replace('.', "") + &number[last..];
    }
    let n: f64 = if number.is_empty() {
        0.0
    } else {
        number.parse().map_err(|_| "Invalid size")?
    };
    let unit: String = v
        .chars()
        .filter(char::is_ascii_alphabetic)
        .filter(|c| *c != 'i')
        .flat_map(char::to_lowercase)
        .collect();
    let power = if unit.contains("kb") {
        1
    } else if unit.contains("mb") {
        2
    } else if unit.contains("gb") {
        3
    } else if unit.contains("tb") {
        4
    } else if unit.is_empty() || unit == "b" {
        0
    } else {
        return Err("Unsupported size unit".into());
    };
    let bytes = n * 1024_f64.powi(power);
    if !bytes.is_finite() || bytes < 0.0 || bytes >= u64::MAX as f64 {
        return Err("Size overflow".into());
    }
    Ok(bytes as u64)
}
fn validate_templates(node: &Node, path: &str) -> Result<(), ExecutionError> {
    if let Some(text) = node.text() {
        if text.contains("{{") {
            Template::compile(text).map_err(|e| error(path, e))?;
        }
    }
    for (k, v) in node.pairs().unwrap_or_default() {
        validate_templates(v, &format!("{path}.{}", k.text().unwrap_or_default()))?;
    }
    for (i, v) in node.items().unwrap_or_default().iter().enumerate() {
        validate_templates(v, &format!("{path}[{i}]"))?;
    }
    Ok(())
}
/// Convert an execution failure into the checker's established source-path diagnostic shape.
impl From<ExecutionError> for Diagnostic {
    fn from(e: ExecutionError) -> Self {
        Self {
            severity: Severity::Error,
            code: "execution_unsupported".into(),
            path: e.path,
            location: None,
            message: e.message,
        }
    }
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn jackett_numeric_normalization_handles_1337x_badges_and_decimal_separators() {
        for (text, value, power) in [
            ("1.6 GB9", 1.69, 3),
            ("1,004.8 MB35", 1004.835, 2),
            (" 3.5 gb ", 3.5, 3),
            ("3,5GB", 3.5, 3),
            ("1.018,29 MB", 1018.29, 2),
            ("1.018.29mb", 1018.29, 2),
            ("512 MiB", 512.0, 2),
            ("1024", 1024.0, 0),
            ("---", 0.0, 0),
        ] {
            assert_eq!(
                super::parse_size(text).unwrap(),
                (value * 1024_f64.powi(power)) as u64,
                "{text}"
            );
        }
        assert!(super::parse_size("9999999999999999999999999999999999999 TB").is_err());
    }
}
