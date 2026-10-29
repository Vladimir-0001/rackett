//! Form controls from the pinned AngleSharp/Jackett form-login path.
//! Values are credentials or CSRF tokens: deliberately no Debug or error echo.
use super::{css, selector};
use scraper::ElementRef;

pub(crate) struct Controls {
    pub action: String,
    pub fields: Vec<(String, String)>,
}
fn html(element: ElementRef<'_>, name: &str) -> bool {
    &*element.value().name.ns == "http://www.w3.org/1999/xhtml" && element.value().name() == name
}
fn disabled(input: ElementRef<'_>) -> bool {
    if input.value().attr("disabled").is_some() {
        return true;
    }
    // AngleSharp HtmlFormControlElement.IsFieldsetDisabled: the nearest
    // disabled fieldset decides, with its first direct legend exempted.
    for fieldset in input.ancestors().filter_map(ElementRef::wrap) {
        if html(fieldset, "fieldset") && fieldset.value().attr("disabled").is_some() {
            let legend = fieldset
                .children()
                .filter_map(ElementRef::wrap)
                .find(|e| html(*e, "legend"));
            return legend.is_none_or(|legend| !input.ancestors().any(|n| n.id() == legend.id()));
        }
    }
    false
}
fn present(doc: &scraper::Html, source: &str) -> Result<bool, String> {
    let css = css::Css::parse(source)?;
    let local = css::check_presence_document(doc, &css)?;
    selector::document_first_filtered(doc, &css, local).map(|e| e.is_some())
}
pub(crate) fn check_errors(document: &str, errors: &[String]) -> Result<(), String> {
    let doc = css::parse_presence_document(document)?;
    selector::check_document(&doc)?;
    for error in errors {
        if present(&doc, error)? {
            return Err("Source rejected form login".into());
        }
    }
    Ok(())
}
pub(crate) fn extract(
    document: &str,
    form_selector: &str,
    captcha: Option<&str>,
) -> Result<Controls, String> {
    if document.len() > 8 * 1024 * 1024 {
        return Err("Login form exceeds byte budget".into());
    }
    let doc = css::parse_presence_document(document)?;
    selector::check_document(&doc)?;
    if captcha.is_some_and(|s| present(&doc, s).unwrap_or(true))
        || present(&doc, "script[src*=simpleCaptcha]")?
    {
        return Err("Login requires an interactive CAPTCHA".into());
    }
    let css = css::Css::parse(form_selector)?;
    let local = css::check_presence_document(&doc, &css)?;
    let form = selector::document_first_filtered(&doc, &css, local)?
        .filter(|e| html(*e, "form"))
        .ok_or("Expected login form missing")?;
    if form.value().attr("enctype").is_some_and(|v| {
        !v.is_empty() && !v.eq_ignore_ascii_case("application/x-www-form-urlencoded")
    }) {
        return Err("Unsupported login form encoding".into());
    }
    let action = form.value().attr("action").unwrap_or("").to_owned();
    if action.len() > 2048 {
        return Err("Login form action exceeds limit".into());
    }
    // A checked radio outside the form can change the checkedness of an input
    // inside it through form=id. Jackett does not collect that external input.
    // Reject this uncommon ownership shape rather than submit a stale choice.
    if let Some(id) = form.value().attr("id") {
        for input in doc.tree.nodes().filter_map(ElementRef::wrap) {
            if html(input, "input")
                && input.value().attr("form") == Some(id)
                && input.value().attr("checked").is_some()
                && input
                    .value()
                    .attr("type")
                    .is_some_and(|t| t.eq_ignore_ascii_case("radio"))
                && !input
                    .ancestors()
                    .any(|n| n.id() == form.id() || n.value().is_fragment())
            {
                return Err("External form radio ownership unsupported".into());
            }
        }
    }
    let mut inputs = Vec::new();
    for input in form.descendants().filter_map(ElementRef::wrap) {
        if !html(input, "input")
            || input.ancestors().any(|n| {
                n.value().is_fragment() || ElementRef::wrap(n).is_some_and(|e| html(e, "select"))
            })
        {
            continue;
        }
        let Some(name) = input.value().attr("name") else {
            continue;
        };
        if input
            .value()
            .attr("form")
            .is_some_and(|owner| Some(owner) != form.value().attr("id"))
        {
            return Err("External form ownership unsupported".into());
        }
        let typ = input.value().attr("type").unwrap_or("text");
        let radio = typ.eq_ignore_ascii_case("radio");
        if (radio || typ.eq_ignore_ascii_case("checkbox"))
            && input.value().attr("checked").is_none()
        {
            continue;
        }
        let value = input.value().attr("value").unwrap_or("");
        if name.len() > 256 || value.len() > 8192 || inputs.len() >= 128 {
            return Err("Login form controls exceed budget".into());
        }
        inputs.push((name, value, radio, disabled(input)));
    }
    let mut fields = Vec::new();
    for (index, (name, value, radio, disabled)) in inputs.iter().enumerate() {
        // Checkedness is established by the parser before Jackett excludes
        // disabled controls. A later disabled radio still unchecks an earlier
        // enabled member. Empty names do not form a radio group upstream.
        if *disabled
            || (*radio
                && !name.is_empty()
                && inputs[index + 1..]
                    .iter()
                    .any(|(n, _, r, _)| *r && n == name))
        {
            continue;
        }
        set(&mut fields, name, value)?;
    }
    Ok(Controls { action, fields })
}
pub(crate) fn set(
    fields: &mut Vec<(String, String)>,
    name: &str,
    value: &str,
) -> Result<(), String> {
    if name.len() > 256 || value.len() > 8192 {
        return Err("Login form controls exceed budget".into());
    }
    if let Some((_, old)) = fields.iter_mut().find(|(key, _)| key == name) {
        *old = value.to_owned();
    } else {
        fields.push((name.to_owned(), value.to_owned()));
    }
    if fields.len() > 128 || fields.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > 65536 {
        return Err("Login form controls exceed budget".into());
    }
    Ok(())
}
