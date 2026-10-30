//! XML into the same bounded tree as HTML, without HTML tree repair or decoding.
//! See docs/xml-responses.md for the AngleSharp.Xml 1.2.0 compatibility contract.
use html5ever::{Attribute, QualName};
use scraper::{node, ElementRef, Html, Node};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_NODES: u32 = 100_000;
const MARKER: &str = "rackett-xml-document";
const CDATA: &str = "rackett-xml-cdata";

fn parse(source: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        source,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
        },
    )
    .map_err(|_| "Invalid XML, unsupported DTD, or XML node limit exceeded".into())
}

pub(super) fn is_xml(element: &ElementRef<'_>) -> bool {
    element.tree().root().first_child().is_some_and(|n| {
        n.value()
            .as_processing_instruction()
            .is_some_and(|pi| pi.target.as_ref() == MARKER)
    })
}

/// CDATA contributes to XmlElement.TextContent, but is not IText in AngleSharp:
/// TextContainsMatcher ignores it. Keep it distinct from ordinary text.
pub(super) fn text(node: &Node) -> Option<&str> {
    node.as_text().map(|t| t.text.as_ref()).or_else(|| {
        node.as_processing_instruction()
            .filter(|pi| pi.target.as_ref() == CDATA)
            .map(|pi| pi.data.as_ref())
    })
}

pub(super) fn parse_document(source: &str) -> Result<Html, String> {
    if source.len() > MAX_BYTES {
        return Err("XML document byte budget exceeded".into());
    }
    // roxmltree merges adjacent CDATA and text. Replace only CDATA tokens with
    // numbered PIs, retaining their data separately. This also avoids quadratic
    // concatenation of many adjacent sections inside roxmltree. Validate CDATA
    // characters here and its location below; everything else goes to the parser.
    // The temporary representation is <= 12 MiB, with <= 100,000 sections.
    let (rewritten, cdata) = separate_cdata(source)?;
    let document = parse(&rewritten)?;
    let mut html = Html::new_document();
    html.tree
        .root_mut()
        .append(Node::ProcessingInstruction(node::ProcessingInstruction {
            target: MARKER.into(),
            data: "".into(),
        }));
    let root = html.tree.root().id();
    let mut parents = vec![root];
    for node in document.descendants().skip(1) {
        let depth = node.ancestors().count() - 1;
        if depth > 128 {
            return Err("XML document depth limit exceeded".into());
        }
        parents.truncate(depth);
        let parent = *parents.last().ok_or("XML parent missing")?;
        let value = match node.node_type() {
            roxmltree::NodeType::Element => {
                let raw = &rewritten[node.range()];
                let qname = raw[1..]
                    .split([' ', '\t', '\r', '\n', '/', '>'])
                    .next()
                    .unwrap_or_default();
                let (prefix, local) = qname
                    .split_once(':')
                    .map_or((None, qname), |(p, l)| (Some(p.into()), l));
                if !qname.is_ascii() {
                    return Err(
                        "Non-ASCII XML element names unsupported (.NET OrdinalIgnoreCase)".into(),
                    );
                }
                // XmlDocument.CreateElementFrom preserves prefix/local spelling but
                // creates XmlElement with null namespace, even with xmlns declarations.
                let name = QualName::new(prefix, "".into(), local.into());
                let mut attrs = Vec::new();
                for attr in node.attributes() {
                    // GetAttribute(name) compares the qualified name, ordinally.
                    let name = &rewritten[attr.range_qname()];
                    attrs.push(Attribute {
                        name: QualName::new(None, "".into(), name.into()),
                        value: attr.value().trim().into(),
                    });
                }
                // roxmltree stores xmlns separately. Preserve locally declared
                // attributes too (including redundant declarations).
                for name in declaration_names(raw) {
                    let prefix = name.strip_prefix("xmlns:");
                    let value = node.lookup_namespace_uri(prefix).unwrap_or_default();
                    attrs.push(Attribute {
                        name: QualName::new(None, "".into(), name.into()),
                        value: value.trim().into(),
                    });
                }
                Node::Element(node::Element::new(name, attrs))
            }
            roxmltree::NodeType::Text => Node::Text(node::Text {
                text: node.text().unwrap_or_default().into(),
            }),
            roxmltree::NodeType::Comment => Node::Comment(node::Comment {
                comment: node.text().unwrap_or_default().into(),
            }),
            roxmltree::NodeType::PI => {
                let pi = node.pi().ok_or("XML processing instruction missing")?;
                let data = if pi.target == CDATA {
                    if !node.parent().is_some_and(|parent| parent.is_element()) {
                        return Err("Invalid XML CDATA outside root element".into());
                    }
                    let index: usize = pi
                        .value
                        .unwrap_or_default()
                        .parse()
                        .map_err(|_| "XML CDATA index invalid")?;
                    cdata.get(index).ok_or("XML CDATA index missing")?.as_str()
                } else {
                    pi.value.unwrap_or_default()
                };
                Node::ProcessingInstruction(node::ProcessingInstruction {
                    target: pi.target.into(),
                    data: data.into(),
                })
            }
            roxmltree::NodeType::Root => return Err("Unexpected XML root".into()),
        };
        let id = html
            .tree
            .get_mut(parent)
            .ok_or("XML parent missing")?
            .append(value)
            .id();
        parents.push(id);
    }
    super::selector::check_document(&html)?;
    Ok(html)
}

/// The input has already passed the XML parser; quotes and token terminators exist.
fn declaration_names(tag: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest =
        tag[1..].trim_start_matches(|c: char| !c.is_ascii_whitespace() && c != '>' && c != '/');
    loop {
        rest = rest.trim_start();
        if rest.starts_with(['>', '/']) {
            break;
        }
        let Some((name, value)) = rest.split_once('=') else {
            break;
        };
        let name = name.trim_end();
        let value = value.trim_start();
        let Some(quote) = value.chars().next() else {
            break;
        };
        let Some(end) = value[1..].find(quote) else {
            break;
        };
        if name == "xmlns" || name.starts_with("xmlns:") {
            names.push(name);
        }
        rest = &value[end + 2..];
    }
    names
}

fn separate_cdata(source: &str) -> Result<(String, Vec<String>), String> {
    let mut output = String::with_capacity(source.len());
    let mut sections = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let (end, replacement) = if let Some(data) = rest.strip_prefix("<![CDATA[") {
            let end = data.find("]]>").ok_or("Unterminated XML CDATA")?;
            if sections.len() >= MAX_NODES as usize {
                return Err("XML CDATA node limit exceeded".into());
            }
            let data = &data[..end];
            if !data.chars().all(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
                return Err("Invalid XML character in CDATA".into());
            }
            sections.push(data.replace("\r\n", "\n").replace('\r', "\n"));
            (
                end + 12,
                Some(format!("<?{CDATA} {}?>", sections.len() - 1)),
            )
        } else if rest.starts_with("<!--") {
            (
                rest.find("-->").ok_or("Unterminated XML comment")? + 3,
                None,
            )
        } else if rest.starts_with("<?") {
            if rest.starts_with("<?rackett-") {
                return Err("Reserved XML processing instruction target".into());
            }
            (
                rest.find("?>")
                    .ok_or("Unterminated XML processing instruction")?
                    + 2,
                None,
            )
        } else if rest.starts_with("<!DOCTYPE") {
            return Err("Invalid XML or unsupported DTD".into());
        } else {
            let mut quote = None;
            let end = rest
                .char_indices()
                .find_map(|(i, c)| {
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        }
                    } else if c == '\'' || c == '"' {
                        quote = Some(c);
                    } else if c == '>' {
                        return Some(i + 1);
                    }
                    None
                })
                .ok_or("Unterminated XML tag")?;
            (end, None)
        };
        output.push_str(replacement.as_deref().unwrap_or(&rest[..end]));
        rest = &rest[end..];
    }
    output.push_str(rest);
    Ok((output, sections))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cardigann_exec::{
        css::{Css, Scan},
        selector,
    };

    fn names(source: &str, selector: &str) -> Vec<String> {
        let document = parse_document(source).unwrap();
        let css = Css::parse(selector).unwrap();
        let scan = Scan::default();
        document
            .tree
            .nodes()
            .filter_map(ElementRef::wrap)
            .filter(|e| css.matches(e, &scan).unwrap())
            .map(|e| e.value().name().to_owned())
            .collect()
    }

    #[test]
    fn tag_names_are_preserved_but_type_selectors_fold_ascii_case() {
        let source = "<RSS><channel><pubDate/><pubdate/><pubDate/></channel></RSS>";
        assert_eq!(
            names(source, "rss > CHANNEL > PUBDATE"),
            ["pubDate", "pubdate", "pubDate"]
        );
        // nth-of-type compares NodeName ordinally, preserving the case distinction.
        assert_eq!(names(source, "pubdate:nth-of-type(2)"), ["pubDate"]);
        assert_eq!(names(source, ":root"), ["RSS"]);
    }

    #[test]
    fn attributes_use_case_sensitive_qualified_names_and_decoded_trimmed_values() {
        let source = r#"<rss xmlns:t="urn:torznab"><t:attr Name=" Seed &amp; peer " value="12" t:Value="x"/></rss>"#;
        assert_eq!(names(source, r#"attr[Name="Seed & peer"]"#), ["attr"]);
        assert!(names(source, "attr[name]").is_empty());
        assert_eq!(names(source, r"attr[t\:Value=x]"), ["attr"]);
        assert!(names(source, "attr[Value]").is_empty());
        let doc = parse_document(source).unwrap();
        let element = doc
            .tree
            .nodes()
            .filter_map(ElementRef::wrap)
            .next_back()
            .unwrap();
        assert_eq!(element.value().attr("Name"), Some("Seed & peer"));
        assert_eq!(element.value().attr("t:Value"), Some("x"));
        assert_eq!(
            doc.root_element().value().attr("xmlns:t"),
            Some("urn:torznab")
        );
    }

    #[test]
    fn namespaces_keep_prefix_and_local_name_without_html_or_namespace_tree_repair() {
        let source = r#"<rss xmlns="urn:rss" xmlns:t="urn:torznab" xmlns:u="urn:torznab"><channel><t:attr name="seeders"/><u:attr name="peers"/><link>url</link><title><b>x</b></title></channel></rss>"#;
        assert_eq!(names(source, "rss channel > attr[name]"), ["attr", "attr"]);
        assert_eq!(names(source, "attr:first-of-type"), ["attr", "attr"]);
        assert!(names(source, r"t\:attr").is_empty());
        assert_eq!(names(source, "*|attr"), ["attr", "attr"]);
        assert_eq!(names(source, "|attr"), ["attr", "attr"]);
        assert_eq!(names(source, "title > b"), ["b"]);
        assert_eq!(names(source, "link:contains(url)"), ["link"]);
        assert!(Css::parse("t|attr").is_err()); // same unsupported dialect as HTML
    }

    #[test]
    fn cdata_is_field_text_but_not_contains_text_and_empty_uses_text_content() {
        let source = "<rss><x>A<![CDATA[<b>&amp;</b>]]>B<!--omit-->C</x><y><![CDATA[data]]></y><z><![CDATA[]]></z></rss>";
        assert_eq!(names(source, "x:contains(ABC)"), ["x"]);
        assert!(names(source, "x:contains(amp)").is_empty());
        assert!(names(source, "y:empty").is_empty());
        assert_eq!(names(source, "z:empty"), ["z"]);
        let document = parse_document(source).unwrap();
        let field =
            crate::cardigann::parse("selector: x", crate::cardigann::Limits::default()).unwrap();
        let field = selector::Selector::compile(&field).unwrap();
        assert_eq!(
            field
                .extract_html(document.root_element(), &|s| Ok(s.into()))
                .unwrap()
                .as_deref(),
            Some("A<b>&amp;</b>BC")
        );
        assert_eq!(
            names(
                "<r><!--<![CDATA[comment]]>--><?test data?><x>&lt;&amp;&#65;&#x42;</x></r>",
                "x:contains('<&AB')"
            ),
            ["x"]
        );
    }

    #[test]
    fn malformed_and_external_entity_inputs_fail_without_exposing_response_text() {
        for source in [
            "<r>",
            "<r><x></r>",
            "<r a='1' a='2'/>",
            "<r>&nbsp;</r>",
            "<r><p:x/></r>",
            "<r><![CDATA[\0]]></r>",
            "<![CDATA[x]]><r/>",
            "<r/>&#0;",
            "<é/>",
            "<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><r>&x;</r>",
            "<?rackett-xml-cdata 0?><r/>",
        ] {
            assert!(parse_document(source).is_err(), "{source:?}");
        }
    }

    #[test]
    fn byte_node_depth_and_cdata_work_bounds_are_enforced() {
        assert!(parse_document(&" ".repeat(MAX_BYTES + 1))
            .unwrap_err()
            .contains("byte budget"));
        let deep = format!("{}{}", "<x>".repeat(129), "</x>".repeat(129));
        assert!(parse_document(&deep).unwrap_err().contains("depth limit"));
        let wide = format!("<r>{}</r>", "<x/>".repeat(MAX_NODES as usize));
        assert!(parse_document(&wide).unwrap_err().contains("node limit"));
        let sections = format!("<r>{}</r>", "<![CDATA[x]]>".repeat(MAX_NODES as usize + 1));
        assert!(parse_document(&sections)
            .unwrap_err()
            .contains("node limit"));
    }
}
