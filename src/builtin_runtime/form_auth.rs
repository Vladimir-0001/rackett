//! Bounded password-form and declared credential-POST plans. Catalog admission still requires per-source runtime fixtures.
//! The transport uses the same scoped, durable cookie store as supplied-cookie sources.
use crate::{
    cardigann::Node,
    cardigann_exec::{css::Css, login_form},
};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct FormPlan {
    pub(super) direct_post: bool,
    pub(super) path: String,
    pub(super) test_path: String,
    pub(super) test_selector: String,
    form: String,
    submit_path: Option<String>,
    inputs: Vec<(String, Input)>,
    errors: Vec<String>,
    captcha: Option<String>,
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
        if root
            .string("encoding")
            .is_some_and(|v| !v.eq_ignore_ascii_case("utf-8"))
            || root.get("encoding").is_some_and(|v| v.text().is_none())
            || ["followredirect", "certificates", "download"]
                .iter()
                .any(|k| root.get(k).is_some())
        {
            bail!("Unsupported form source transport");
        }
        let search = root
            .get("search")
            .ok_or_else(|| anyhow!("Missing form source search"))?;
        if search.pairs().is_none()
            || search.get("headers").is_some()
            || search.get("response").is_some()
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
                || path.get("response").is_some()
                || path.get("followredirect").is_some()
                || path
                    .get("method")
                    .is_some_and(|m| !m.text().is_some_and(|m| m.eq_ignore_ascii_case("get")))
            {
                bail!("Form sessions require plain HTML GET searches");
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
                ],
            )?;
        }
        let test = login
            .get("test")
            .ok_or_else(|| anyhow!("Form login needs a positive test"))?;
        keys(test, &["path", "selector"])?;
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
                    keys(message, &["text", "selector", "attribute", "filters"])?;
                }
                errors.push(selector_property(block, "selector", None)?);
            }
        }
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
            test_path: literal(test, "path", None)?,
            test_selector: selector_property(test, "selector", None)?,
            inputs,
            errors,
            captcha,
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
