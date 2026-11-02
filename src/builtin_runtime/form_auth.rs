//! Bounded password-form and declared credential-POST plans. Catalog admission still requires per-source runtime fixtures.
//! The transport uses the same scoped, durable cookie store as supplied-cookie sources.
use crate::{
    cardigann::Node,
    cardigann_exec::{css::Css, login_form, request::Charset},
};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct FormPlan {
    pub(super) direct_post: bool,
    pub(super) charset: Charset,
    pub(super) path: String,
    /// `login.test`; absent or selector-less tests are pinned semantics, not defaults.
    pub(super) test: Option<super::auth::LoginTest>,
    form: String,
    submit_path: Option<String>,
    inputs: Vec<(String, Input)>,
    errors: Vec<String>,
    captcha: Option<String>,
    /// `login.selectorinputs`: form fields copied from the landing page, set after
    /// the declared inputs (`CardigannIndexer.cs:678-699`).
    selector_inputs: Vec<(
        String,
        crate::cardigann_exec::selector::Selector,
        Vec<crate::cardigann_exec::filters::Filter>,
    )>,
    /// `login.cookies`, joined: the cookie seed sent before fetching the login form
    /// (`CardigannIndexer.cs:1026`).
    pub(super) cookies: Option<String>,
    /// Definition `certificates`, pinned for the session's origin host only.
    pub(super) certificates: Option<crate::pinned_tls::Pins>,
    /// `search.headers`, also sent with the login requests (`Login.Headers ??
    /// Search.Headers`, docs/session-search-headers.md).
    pub(super) headers: Vec<super::auth::DeclaredHeader>,
}
enum Input {
    Literal(String),
    Setting(String),
    Select {
        name: String,
        default: String,
        options: Vec<String>,
    },
}
fn keys(node: &Node, allowed: &[&str]) -> Result<()> {
    let pairs = node
        .pairs()
        .ok_or_else(|| anyhow!("Expected form-login mapping"))?;
    if pairs
        .iter()
        .any(|(k, _)| !k.text().is_some_and(|k| allowed.contains(&k)))
    {
        bail!("Unsupported form-login property");
    }
    Ok(())
}
fn literal(node: &Node, key: &str, fallback: Option<&str>) -> Result<String> {
    let value = match node.get(key) {
        Some(n) => n
            .text()
            .ok_or_else(|| anyhow!("Expected form-login scalar"))?,
        None => fallback.ok_or_else(|| anyhow!("Missing form-login property"))?,
    };
    if value.len() > 4096 || value.contains(['{', '}']) || value.chars().any(char::is_control) {
        bail!("Unsupported form-login literal");
    }
    Ok(value.to_owned())
}
fn selector(value: String) -> Result<String> {
    if value.is_empty() || Css::parse(&value).is_err() {
        bail!("Unsupported form-login selector");
    }
    Ok(value)
}

fn selector_property(node: &Node, key: &str, fallback: Option<&str>) -> Result<String> {
    let value = match node.get(key) {
        Some(n) => n
            .text()
            .ok_or_else(|| anyhow!("Expected form-login scalar"))?,
        None => fallback.ok_or_else(|| anyhow!("Missing form-login property"))?,
    };
    // CSS strings can contain literal JSON braces (DiabloTorrent's login error).
    // These selectors are never rendered as templates; template syntax stays blocked.
    if value.len() > 4096
        || value.contains("{{")
        || value.contains("}}")
        || value.chars().any(char::is_control)
    {
        bail!("Unsupported form-login literal");
    }
    selector(value.to_owned())
}
impl FormPlan {
    pub fn compile(root: &Node) -> Result<Self> {
        super::auth::unique_mappings(root)?;
        let charset = Charset::from_definition(root)
            .map_err(|_| anyhow!("Unsupported form source transport"))?;
        // A download block is admitted only in the general selector shape; its detail
        // pages and files are then fetched through this session (see download.rs).
        // Well-formed pins are installed on this session's client (pinned_tls.rs).
        let certificates = crate::pinned_tls::Pins::compile(root)
            .map_err(|_| anyhow!("Unsupported form source transport"))?;
        if root.get("followredirect").is_some()
            || (root.get("download").is_some()
                && super::download::DownloadSelectors::compile(root).is_err())
        {
            bail!("Unsupported form source transport");
        }
        let search = root
            .get("search")
            .ok_or_else(|| anyhow!("Missing form source search"))?;
        // Search responses may be HTML (default), JSON or XML; RequestPlan owns the
        // rest of the response semantics.
        let response_ok = |n: &Node| {
            n.get("response").is_none_or(|r| {
                r.string("type")
                    .is_none_or(|t| ["html", "json", "xml"].contains(&t))
            })
        };
        // Form sessions bind no User-Agent setting; other declared headers are sent.
        let (agent, headers) = super::auth::compile_search_headers(root)
            .map_err(|_| anyhow!("Unsupported form session headers"))?;
        if agent {
            bail!("Unsupported form session headers");
        }
        if search.pairs().is_none()
            || !response_ok(search)
            || search.get("followredirect").is_some()
        {
            bail!("Form sessions require plain HTML GET searches");
        }
        // The pinned format also has a legacy single search.path. Validate it
        // through the same transport gate; never ignore an ambiguous path list.
        let paths: Vec<&Node> = match (search.get("paths"), search.get("path")) {
            (Some(paths), None) => paths
                .items()
                .ok_or_else(|| anyhow!("Unsupported form source paths"))?
                .iter()
                .collect(),
            (None, Some(path)) if path.text().is_some_and(|p| !p.is_empty()) => {
                vec![search]
            }
            _ => bail!("Missing or ambiguous form source paths"),
        };
        if paths.is_empty() || paths.len() > 64 {
            bail!("Unsupported form source paths");
        }
        for path in paths {
            if path.pairs().is_none()
                || path.get("headers").is_some()
                || !response_ok(path)
                || path.get("followredirect").is_some()
                // HTML GET, or a form POST the request plan encodes upstream's way.
                || path.get("method").is_some_and(|m| {
                    !m.text().is_some_and(|m| {
                        m.eq_ignore_ascii_case("get") || m.eq_ignore_ascii_case("post")
                    })
                })
            {
                bail!("Form sessions require plain HTML GET or form POST searches");
            }
        }
        let login = root
            .get("login")
            .ok_or_else(|| anyhow!("Missing form login"))?;
        let direct_post = match login.get("method") {
            None => false,
            Some(n) if n.text() == Some("form") => false,
            Some(n) if n.text() == Some("post") => true,
            _ => bail!("Expected form or credential-POST login method"),
        };
        if direct_post {
            // POST declarations send only their configured fields. They never fetch
            // a form or silently ignore CAPTCHA, hidden-input or custom-header rules.
            keys(login, &["method", "path", "inputs", "test", "error"])?;
            let declared_inputs = login
                .get("inputs")
                .and_then(Node::pairs)
                .ok_or_else(|| anyhow!("Missing credential-POST inputs"))?;
            for required in ["{{ .Config.username }}", "{{ .Config.password }}"] {
                if !declared_inputs
                    .iter()
                    .any(|(_, value)| value.text() == Some(required))
                {
                    bail!("Credential POST requires declared username and password");
                }
            }
        } else {
            keys(
                login,
                &[
                    "method",
                    "path",
                    "form",
                    "submitpath",
                    "inputs",
                    "test",
                    "error",
                    "captcha",
                    "selectorinputs",
                    "cookies",
                ],
            )?;
        }
        // Pinned TestLogin and CheckIfLoginIsNeeded skip validation without a test and
        // marker checks without a selector (`CardigannIndexer.cs:896-899`, `:935`, `:977`).
        let test = login.get("test");
        if let Some(test) = test {
            keys(test, &["path", "selector"])?;
        }
        let declared: BTreeMap<_, _> = root
            .get("settings")
            .and_then(Node::items)
            .unwrap_or_default()
            .iter()
            .filter_map(|s| s.string("name").map(|name| (name, s)))
            .collect();
        let mut inputs = Vec::new();
        for (name, value) in login
            .get("inputs")
            .and_then(Node::pairs)
            .ok_or_else(|| anyhow!("Missing form credentials"))?
        {
            let name = name
                .text()
                .ok_or_else(|| anyhow!("Invalid form input name"))?;
            let value = value
                .text()
                .ok_or_else(|| anyhow!("Invalid form input value"))?;
            if name.is_empty()
                || name.len() > 256
                || name.chars().any(char::is_control)
                || value.len() > 8192
                || inputs.len() >= 128
            {
                bail!("Form inputs exceed budget");
            }
            let input = if let Some(key) = value
                .strip_prefix("{{ .Config.")
                .and_then(|v| v.strip_suffix(" }}"))
            {
                if key == "sitelink" {
                    Input::Setting(key.to_owned())
                } else {
                    let setting = declared
                        .get(key)
                        .ok_or_else(|| anyhow!("Form input requires a declared setting"))?;
                    match setting.string("type") {
                        Some("text" | "password") => Input::Setting(key.to_owned()),
                        Some("select") => {
                            let options = setting
                                .get("options")
                                .and_then(Node::pairs)
                                .ok_or_else(|| anyhow!("Invalid form select options"))?;
                            if options.is_empty() || options.len() > 128 {
                                bail!("Invalid form select options");
                            }
                            let options: Vec<String> = options
                                .iter()
                                .map(|(k, _)| {
                                    let v = k
                                        .text()
                                        .ok_or_else(|| anyhow!("Invalid form select option"))?;
                                    if v.len() > 8192 || v.chars().any(char::is_control) {
                                        bail!("Invalid form select option");
                                    }
                                    Ok(v.to_owned())
                                })
                                .collect::<Result<_>>()?;
                            let default = literal(setting, "default", None)?;
                            if !options.contains(&default) {
                                bail!("Invalid form select default");
                            }
                            Input::Select {
                                name: key.to_owned(),
                                default,
                                options,
                            }
                        }
                        _ => bail!("Unsupported form setting type"),
                    }
                }
            } else {
                if value.contains(['{', '}']) {
                    bail!("Unsupported form input template");
                }
                Input::Literal(value.to_owned())
            };
            inputs.push((name.to_owned(), input));
        }
        let mut errors = Vec::new();
        if let Some(blocks) = login.get("error") {
            let blocks = blocks
                .items()
                .ok_or_else(|| anyhow!("Expected form error list"))?;
            if blocks.len() > 8 {
                bail!("Too many form error selectors");
            }
            for block in blocks {
                keys(block, &["selector", "message"])?;
                // Upstream raises on the matching selector regardless of the
                // message source. Messages are neither evaluated nor exposed.
                if let Some(message) = block.get("message") {
                    keys(
                        message,
                        &["text", "selector", "attribute", "filters", "remove"],
                    )?;
                }
                errors.push(selector_property(block, "selector", None)?);
            }
        }
        let mut selector_inputs = Vec::new();
        if let Some(map) = login.get("selectorinputs") {
            let map = map
                .pairs()
                .ok_or_else(|| anyhow!("Expected selector input mapping"))?;
            if map.is_empty() || map.len() > 16 {
                bail!("Unsupported selector input count");
            }
            for (name, node) in map {
                // handleSelector fields: a required selector, then attribute or text, then
                // filters. Templates, cases, removal and optional inputs stay unsupported.
                keys(node, &["selector", "attribute", "filters"])?;
                let name = name
                    .text()
                    .filter(|n| !n.is_empty() && n.len() <= 256 && !n.chars().any(char::is_control))
                    .ok_or_else(|| anyhow!("Invalid selector input name"))?;
                selector_property(node, "selector", None)?;
                if node
                    .string("attribute")
                    .is_some_and(|a| a.is_empty() || a.contains("{{"))
                {
                    bail!("Invalid selector input attribute");
                }
                let selector = crate::cardigann_exec::selector::Selector::compile(node)
                    .map_err(|e| anyhow!(e))?;
                selector.validate_html().map_err(|e| anyhow!(e))?;
                let mut filters = Vec::new();
                for f in node
                    .get("filters")
                    .map(|f| f.items().ok_or_else(|| anyhow!("Filters must be a list")))
                    .transpose()?
                    .unwrap_or_default()
                {
                    keys(f, &["name", "args"])?;
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
                    if filters.len() >= 8 {
                        bail!("Selector input filter limit exceeded");
                    }
                    filters.push(
                        crate::cardigann_exec::filters::Filter::compile_for(
                            f.string("name").unwrap_or_default(),
                            &args,
                            charset,
                        )
                        .map_err(|e| anyhow!(e))?,
                    );
                }
                selector_inputs.push((name.to_owned(), selector, filters));
            }
        }
        let cookies = if let Some(list) = login.get("cookies") {
            let list = list
                .items()
                .filter(|l| (1..=8).contains(&l.len()))
                .ok_or_else(|| anyhow!("Unsupported login cookie list"))?;
            let mut pairs = Vec::new();
            for item in list {
                let pair = item
                    .text()
                    .filter(|p| {
                        p.split_once('=').is_some_and(|(k, v)| {
                            !k.is_empty()
                                && k.bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
                                && v.bytes()
                                    .all(|b| b.is_ascii_graphic() && !b";,\\\"".contains(&b))
                        }) && !p.contains("{{")
                    })
                    .ok_or_else(|| anyhow!("Unsupported login cookie"))?;
                pairs.push(pair.to_owned());
            }
            Some(pairs.join("; "))
        } else {
            None
        };
        let captcha = if let Some(c) = login.get("captcha") {
            keys(c, &["type", "selector", "input"])?;
            if !matches!(c.string("type"), Some("image" | "text")) {
                bail!("Unsupported form CAPTCHA type");
            }
            let _ = literal(c, "input", None)?;
            Some(selector_property(c, "selector", None)?)
        } else {
            None
        };
        Ok(Self {
            headers,
            charset,
            direct_post,
            path: literal(login, "path", None)?,
            form: if direct_post {
                String::new()
            } else {
                selector_property(login, "form", Some("form"))?
            },
            submit_path: login
                .get("submitpath")
                .map(|_| literal(login, "submitpath", None))
                .transpose()?,
            test: test
                .map(|test| {
                    Ok::<_, anyhow::Error>(super::auth::LoginTest {
                        path: literal(test, "path", None)?,
                        selector: test
                            .get("selector")
                            .map(|_| selector_property(test, "selector", None))
                            .transpose()?,
                    })
                })
                .transpose()?,
            inputs,
            errors,
            captcha,
            selector_inputs,
            cookies,
            certificates,
        })
    }
    pub(super) fn configure(
        self,
        settings: &BTreeMap<String, Value>,
        base: &str,
    ) -> Result<ConfiguredForm> {
        let mut inputs = Vec::new();
        for (name, input) in &self.inputs {
            let value = match input {
                Input::Literal(v) => v.clone(),
                Input::Setting(k) if k == "sitelink" => base.to_owned(),
                Input::Setting(k) => {
                    let v = match settings.get(k) {
                        Some(Value::String(s)) => s.clone(),
                        None => String::new(),
                        _ => bail!("Form credentials must be text"),
                    };
                    if ["username", "password"].contains(&k.as_str()) && v.is_empty() {
                        bail!("Form preset requires username and password");
                    }
                    v
                }
                Input::Select {
                    name,
                    default,
                    options,
                } => {
                    let v = match settings.get(name) {
                        Some(Value::String(v)) => v,
                        None => default,
                        _ => bail!("Form select value must be text"),
                    };
                    if !options.contains(v) {
                        bail!("Invalid form select value");
                    }
                    v.clone()
                }
            };
            login_form::set(&mut inputs, name, &value)
                .map_err(|_| anyhow!("Form credentials exceed budget"))?;
        }
        Ok(ConfiguredForm { plan: self, inputs })
    }
}
pub(super) struct ConfiguredForm {
    pub plan: FormPlan,
    pub inputs: Vec<(String, String)>,
}
pub(super) struct Submission {
    pub target: String,
    pub fields: Vec<(String, String)>,
}
impl ConfiguredForm {
    pub fn prepare(&self, body: &str, landing: &url::Url) -> Result<Submission> {
        let controls = login_form::extract(body, &self.plan.form, self.plan.captcha.as_deref())
            .map_err(|_| {
                anyhow!("Login form missing, unsupported or requires interactive CAPTCHA")
            })?;
        let mut fields = controls.fields;
        for (name, value) in &self.inputs {
            login_form::set(&mut fields, name, value)
                .map_err(|_| anyhow!("Form credentials exceed budget"))?;
        }
        // Required selector inputs: a missing element or attribute fails the login.
        if !self.plan.selector_inputs.is_empty() {
            let html = crate::cardigann_exec::css::parse_document(body);
            crate::cardigann_exec::css::check_error_document(&html)
                .map_err(|_| anyhow!("Login form token missing"))?;
            let ctx = crate::cardigann_exec::template::Context {
                values: Default::default(),
                now: chrono::Utc::now(),
            };
            for (name, selector, filters) in &self.plan.selector_inputs {
                let mut value = selector
                    .extract_html(html.root_element(), &|s: &str| Ok(s.to_owned()))
                    .ok()
                    .flatten()
                    .ok_or_else(|| anyhow!("Login form token missing"))?;
                for filter in filters {
                    value = filter
                        .apply(&value, &ctx)
                        .map_err(|_| anyhow!("Login form token missing"))?;
                }
                login_form::set(&mut fields, name, &value)
                    .map_err(|_| anyhow!("Form credentials exceed budget"))?;
            }
        }
        let action = self.plan.submit_path.as_deref().unwrap_or(&controls.action);
        let target = landing
            .join(action)
            .map_err(|_| anyhow!("Invalid form action"))?;
        super::checked_url(target.as_str(), landing)?;
        if target.fragment().is_some() {
            bail!("Form action fragment rejected");
        }
        Ok(Submission {
            target: target.to_string(),
            fields,
        })
    }
    pub fn check_response(&self, body: &str) -> Result<()> {
        login_form::check_errors(body, &self.plan.errors)
            .map_err(|_| anyhow!("Source rejected form login or returned unsupported HTML"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{builtin, cardigann};
    fn plan(id: &str) -> FormPlan {
        let raw = builtin::SOURCES.iter().find(|(i, _)| *i == id).unwrap().1;
        FormPlan::compile(&cardigann::parse(raw, cardigann::Limits::default()).unwrap()).unwrap()
    }
    fn credentials() -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("username".into(), Value::String("u".into())),
            ("password".into(), Value::String("p".into())),
        ])
    }
    #[test]
    fn selector_inputs_copy_attributes_and_filtered_text_after_declared_inputs() {
        let landing = url::Url::parse("https://fixture.test/?p=home&pid=1").unwrap();
        // BitTurk: text of a script, through the pinned regexp filter.
        let form = plan("bitturk")
            .configure(&credentials(), "https://fixture.test/")
            .unwrap();
        let page = "<form id=loginbox_form action=x><input name=a value=b></form>\
                    <script>var c = {stKey: \"tok-123\", other: 1};</script>";
        let fields = form.prepare(page, &landing).unwrap().fields;
        assert!(fields.contains(&("securitytoken".into(), "tok-123".into())));
        assert!(fields.contains(&("loginbox_membername".into(), "u".into())));
        // A missing token fails the login before any credential is sent.
        assert!(form
            .prepare("<form id=loginbox_form action=x></form>", &landing)
            .is_err());
        // HDC: two hidden attributes.
        let form = plan("hdc")
            .configure(&credentials(), "https://fixture.test/")
            .unwrap();
        let form_selector = form.plan.form.clone();
        assert!(!form_selector.is_empty());
    }
    #[test]
    fn absent_or_selector_less_login_tests_are_explicit_not_defaulted() {
        for id in ["comicat", "immortuos", "miobt", "zappateers"] {
            assert!(plan(id).test.is_none(), "{id}");
        }
        let test = plan("4thd").test.unwrap();
        assert!(!test.path.is_empty() && test.selector.is_some());
        let raw = builtin::SOURCES
            .iter()
            .find(|(i, _)| *i == "miobt")
            .unwrap()
            .1;
        let with = |test: &str| {
            let yaml = raw.replace(
                "  inputs:\n    visitor_test: human\n",
                &format!("  inputs:\n    visitor_test: human\n  test:\n{test}"),
            );
            assert_ne!(yaml, raw);
            FormPlan::compile(&cardigann::parse(&yaml, cardigann::Limits::default()).unwrap())
        };
        let test = with("    path: index.php\n").unwrap().test.unwrap();
        assert_eq!((test.path.as_str(), test.selector), ("index.php", None));
        for bad in [
            "    selector: a\n",
            "    path: index.php\n    selector: ''\n",
            "    path: index.php\n    other: x\n",
        ] {
            assert!(with(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn login_cookie_seeds_are_joined_and_restricted() {
        assert_eq!(plan("coastalcrew").cookies.as_deref(), Some("JAVA=OK"));
        assert_eq!(plan("chdbits").cookies, None);
    }
}
