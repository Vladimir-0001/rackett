//! scraper's CSS selector dialect plus AngleSharp's text `:contains(...)`.
//!
//! HTML built-in selector features delegate to scraper's `selectors::Element`;
//! XML additionally overrides local-name matching and `:empty` (see `xml.rs`).
//! The `:contains` pseudo-class is added. Source: AngleSharp v1.8.6
//! (9253950e551f9ed95294f775675de9aa249894a2),
//! `Css/Parser/CssSelectorConstructor.cs:25-43,376,958-1000` (case-insensitive
//! function name, ident or string argument) and
//! `Css/Dom/Internal/TextContainsMatcher.cs:28-94` (ordinal KMP over descendant
//! text nodes, across node boundaries, comments excluded, empty needle matches).
//! Template contents are outside `ChildNodes` upstream
//! (`Html/Dom/Internal/HtmlTemplateElement.cs:60-94`,
//! `Html/Parser/HtmlDomBuilder.cs:2623-2640,3034-3059`) and are skipped here.
//!
//! Known limits outside this module's text model (separate DOM-parity work):
//! selection and field text extraction still descend into inert template
//! contents; a template that AngleSharp turns into a shadow root stays in
//! scraper's tree as an element; and ordinary selectors keep html5ever's
//! dropped-shadow-root tree. `:contains` itself fails explicitly on that tree
//! (see [`parse_document`]). `search.error` checks instead fail closed on those
//! documents and on select elements for every selector (see
//! [`check_error_document`]).
use cssparser::{ParserInput, ToCss, Token};
use html5ever::{
    driver::ParseOpts,
    tendril::{StrTendril, TendrilSink},
    tree_builder::{
        ElementFlags, NextParserState, NodeOrText, QuirksMode, TreeBuilderOpts, TreeSink,
    },
    Attribute, QualName,
};
use scraper::{
    error::SelectorErrorKind,
    node::ProcessingInstruction,
    selector::{CssLocalName, CssString, Simple},
    CaseSensitivity, ElementRef, Html, HtmlTreeSink,
};
use selectors::{
    attr::{AttrSelectorOperation, NamespaceConstraint},
    bloom::BloomFilter,
    matching::{self, ElementSelectorFlags, MatchingContext},
    parser::{self, ParseRelative, SelectorImpl, SelectorList, SelectorParseErrorKind},
    OpaqueElement,
};
use std::{borrow::Cow, cell::Cell, fmt};

/// Units (descendant nodes visited plus text bytes scanned) one selection pass,
/// removal pass or case evaluation may spend on `:contains`. There is
/// deliberately no response-wide text budget: total work is this per-pass bound
/// times the existing row, field, case and aggregate-visit limits.
pub const MAX_CONTAINS_SCAN: usize = 32 * 1024 * 1024;

/// Prefix of every `:contains` execution failure. These describe the response,
/// not one field, so callers must not treat them as an optional field miss.
const FAILURE_PREFIX: &str = "CSS :contains ";
const BUDGET_EXCEEDED: &str = "CSS :contains text scan budget exceeded";
const DROPPED_SHADOW_ROOT: &str =
    "CSS :contains unsupported in a document with a dropped declarative shadow root template";
/// Processing-instruction target marking a document whose text cannot match
/// upstream (see [`parse_document`]).
const DROPPED_SHADOW_ROOT_MARKER: &str = "rackett-dropped-declarative-shadow-root";

/// Whether an HTML selection error is a `:contains` execution failure (scan
/// budget or unsupported document) that must fail the whole response.
pub fn is_execution_failure(message: &str) -> bool {
    message.starts_with(FAILURE_PREFIX)
}

/// Per-pass `:contains` budget. Exhaustion and unsupported traversal are sticky
/// and reported as errors by [`Css::matches`], so a failed scan never becomes a
/// silent non-match (including below `:not`, `:has` or in case keys).
#[derive(Debug)]
pub struct Scan {
    left: Cell<usize>,
    failure: Cell<Option<&'static str>>,
}
impl Default for Scan {
    fn default() -> Self {
        Self::with_limit(MAX_CONTAINS_SCAN)
    }
}
impl Scan {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            left: Cell::new(limit),
            failure: Cell::new(None),
        }
    }
    fn charge(&self, units: usize) -> bool {
        match self.left.get().checked_sub(units) {
            Some(left) if self.failure.get().is_none() => {
                self.left.set(left);
                true
            }
            _ => self.fail(BUDGET_EXCEEDED),
        }
    }
    fn fail(&self, reason: &'static str) -> bool {
        if self.failure.get().is_none() {
            self.failure.set(Some(reason));
        }
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Impl;
impl SelectorImpl for Impl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssString;
    type Identifier = CssLocalName;
    type LocalName = CssLocalName;
    type NamespacePrefix = CssLocalName;
    type NamespaceUrl = <Simple as SelectorImpl>::NamespaceUrl;
    type BorrowedNamespaceUrl = <Simple as SelectorImpl>::BorrowedNamespaceUrl;
    type BorrowedLocalName = CssLocalName;
    type NonTSPseudoClass = Contains;
    type PseudoElement = PseudoElement;
}

/// `:contains(text)` with its KMP prefix table built once at parse time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contains {
    needle: String,
    prefixes: Vec<usize>,
}
impl Contains {
    fn new(needle: String) -> Self {
        let bytes = needle.as_bytes();
        let mut prefixes = vec![0; bytes.len()];
        let mut k = 0;
        for i in 1..bytes.len() {
            while k > 0 && bytes[i] != bytes[k] {
                k = prefixes[k - 1];
            }
            if bytes[i] == bytes[k] {
                k += 1;
            }
            prefixes[i] = k;
        }
        Self { needle, prefixes }
    }
    /// UTF-8 byte KMP is equivalent to AngleSharp's UTF-16 ordinal KMP for
    /// valid strings: both encodings are prefix-free per scalar value.
    fn matches(&self, element: ElementRef<'_>, scan: &Scan) -> bool {
        let needle = self.needle.as_bytes();
        if needle.is_empty() {
            return true;
        }
        let root = *element;
        let mut k = 0;
        let mut next = Some(root);
        while let Some(node) = next {
            // scraper stores HTML template contents under a Fragment child of
            // the template element; AngleSharp moves them to the inert
            // `template.Content` fragment. Neither is in the parent's text.
            let inert = node.value().is_fragment();
            if !inert {
                if !scan.charge(1) {
                    return false;
                }
                if let Some(text) = node.value().as_text() {
                    // Charge only bytes actually scanned; stop before exceeding the budget.
                    let available = scan.left.get();
                    for (i, &byte) in text.as_bytes().iter().enumerate() {
                        if i == available {
                            return scan.charge(i + 1);
                        }
                        while k > 0 && byte != needle[k] {
                            k = self.prefixes[k - 1];
                        }
                        if byte == needle[k] {
                            k += 1;
                            if k == needle.len() {
                                return scan.charge(i + 1);
                            }
                        }
                    }
                    scan.charge(text.len());
                }
            }
            // Pre-order step without allocation, never leaving `root`.
            next = if inert { None } else { node.first_child() };
            let mut cursor = node;
            while next.is_none() && cursor.id() != root.id() {
                next = cursor.next_sibling();
                match cursor.parent() {
                    Some(parent) => cursor = parent,
                    None => break,
                }
            }
        }
        false
    }
}
impl ToCss for Contains {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(":contains(")?;
        cssparser::serialize_string(&self.needle, dest)?;
        dest.write_str(")")
    }
}
impl parser::NonTSPseudoClass for Contains {
    type Impl = Impl;
    fn is_active_or_hover(&self) -> bool {
        false
    }
    fn is_user_action_state(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudoElement {}
impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, _: &mut W) -> fmt::Result {
        match *self {}
    }
}
impl parser::PseudoElement for PseudoElement {
    type Impl = Impl;
}

struct Parser<'s> {
    source: &'s str,
    contains_error: Cell<Option<&'static str>>,
    /// A non-empty `:contains` occurs anywhere, including nested lists.
    scans_text: Cell<bool>,
}
impl Parser<'_> {
    fn reject<'i, T>(
        &self,
        input: &cssparser::Parser<'i, '_>,
        reason: &'static str,
    ) -> Result<T, cssparser::ParseError<'i, SelectorParseErrorKind<'i>>> {
        self.contains_error.set(Some(reason));
        Err(
            input.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                "contains".into(),
            )),
        )
    }
}
impl<'i> parser::Parser<'i> for Parser<'_> {
    type Impl = Impl;
    type Error = SelectorParseErrorKind<'i>;
    // Same options as scraper::selector::Parser.
    fn parse_is_and_where(&self) -> bool {
        true
    }
    fn parse_has(&self) -> bool {
        true
    }
    /// Approved subset of AngleSharp's `ContainsFunctionState`: exactly one
    /// ident or terminated string token, optionally surrounded by whitespace.
    fn parse_non_ts_functional_pseudo_class<'t>(
        &self,
        name: cssparser::CowRcStr<'i>,
        input: &mut cssparser::Parser<'i, 't>,
        _after_part: bool,
    ) -> Result<Contains, cssparser::ParseError<'i, Self::Error>> {
        if !name.eq_ignore_ascii_case("contains") {
            return Err(input.new_custom_error(
                SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name),
            ));
        }
        // Record the argument token's own start, after whitespace and comments.
        let (start, token) = loop {
            let start = input.position();
            match input.next_including_whitespace_and_comments() {
                Ok(Token::WhiteSpace(_) | Token::Comment(_)) => {}
                Ok(token) => break (start, token.clone()),
                Err(_) => return self.reject(input, "empty argument"),
            }
        };
        let needle = match token {
            Token::Ident(value) => value.to_string(),
            Token::QuotedString(value) => {
                if !terminated_string(input.slice_from(start)) {
                    return self.reject(input, "unterminated string");
                }
                value.to_string()
            }
            _ => return self.reject(input, "argument must be one ident or string"),
        };
        if input.expect_exhausted().is_err() {
            return self.reject(input, "argument must be exactly one token");
        }
        // The nested block ends at its `)` or, leniently, at end of input.
        input.skip_whitespace();
        if !self.source[input.position().byte_index()..].starts_with(')') {
            return self.reject(input, "unterminated function");
        }
        if !needle.is_empty() {
            self.scans_text.set(true);
        }
        Ok(Contains::new(needle))
    }
}

/// Raw quoted-string source ends with an unescaped closing quote.
fn terminated_string(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(quote) = chars.next() else {
        return false;
    };
    while let Some(c) = chars.next() {
        if c == '\\' {
            if chars.next().is_none() {
                return false;
            }
        } else if c == quote {
            return chars.next().is_none();
        }
    }
    false
}

/// Checks one configured value that Jackett substitutes raw as a whole
/// `:contains(...)` argument (`CardigannIndexer.cs:517-530`). Upstream's
/// `ContainsFunctionState` keeps the last ident or string token, so a raw
/// multi-token value silently changes meaning; only exactly one token is
/// accepted here. The subset is one where cssparser and AngleSharp's tokenizer
/// (`CssTokenizer.cs:113-143,551-600`, `CharExtensions.cs:124,149,173`) agree:
/// a plain identifier (ASCII letters, digits, `_`, `-`, non-ASCII; not starting
/// with a digit or `--`) or a quoted string without its own quote. Escapes,
/// controls, line separators and U+FFFF (AngleSharp's end-of-file marker) are
/// rejected. Errors are fixed text and never repeat the value.
pub(super) fn contains_value(value: &str) -> Result<(), &'static str> {
    const INVALID: &str =
        "row selector configuration value must be exactly one CSS identifier or quoted string";
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '\u{2028}' | '\u{2029}' | '\u{FFFF}'))
    {
        return Err(INVALID);
    }
    let name = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || !c.is_ascii();
    let start = |c: char| c.is_ascii_alphabetic() || c == '_' || !c.is_ascii();
    let mut chars = value.chars();
    let first = chars.next().ok_or(INVALID)?;
    let quoted = if first == '"' || first == '\'' {
        let inner = value[1..].strip_suffix(first).ok_or(INVALID)?;
        if inner.contains(first) {
            return Err(INVALID);
        }
        Some(inner)
    } else {
        let lead = if first == '-' {
            chars.next()
        } else {
            Some(first)
        };
        if !lead.is_some_and(start) || !value.chars().all(name) {
            return Err(INVALID);
        }
        None
    };
    // Cross-check the subset against cssparser: one token, then end of input.
    let mut input = ParserInput::new(value);
    let mut input = cssparser::Parser::new(&mut input);
    let token = input
        .next_including_whitespace_and_comments()
        .map_err(|_| INVALID)?
        .clone();
    let same = match (&token, quoted) {
        (Token::Ident(ident), None) => &**ident == value,
        (Token::QuotedString(string), Some(inner)) => &**string == inner,
        _ => false,
    };
    if !same || input.next_including_whitespace_and_comments().is_ok() {
        return Err(INVALID);
    }
    Ok(())
}

/// Whether each range of `source` is exactly the sole argument token of a
/// `:contains(...)`, with no whitespace or comment around it, closed by `)`,
/// and reached only through the top-level selector list or `:not`, `:is`,
/// `:where` and `:has` arguments. Positions come from cssparser's tokenizer,
/// so a range inside a string, comment, attribute selector, other function or
/// a longer token is never proven.
pub(super) fn contains_arguments(source: &str, ranges: &[std::ops::Range<usize>]) -> bool {
    fn walk<'i>(
        input: &mut cssparser::Parser<'i, '_>,
        source: &str,
        ranges: &[std::ops::Range<usize>],
        proven: &mut [bool],
        selectors: bool,
        depth: usize,
    ) -> Result<(), cssparser::ParseError<'i, ()>> {
        let mut colon = false;
        loop {
            let token = match input.next_including_whitespace_and_comments() {
                Ok(token) => token.clone(),
                Err(_) => return Ok(()),
            };
            let pseudo = selectors && colon;
            colon = token == Token::Colon;
            let nested = match &token {
                Token::Function(name) if pseudo && name.eq_ignore_ascii_case("contains") => None,
                Token::Function(name) => Some(
                    pseudo
                        && ["not", "is", "where", "has"]
                            .iter()
                            .any(|f| name.eq_ignore_ascii_case(f)),
                ),
                Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                    Some(false)
                }
                _ => continue,
            };
            if depth >= MAX_REWRITE_DEPTH {
                return Err(input.new_custom_error(()));
            }
            input.parse_nested_block(|block| {
                if let Some(nested) = nested {
                    return walk(block, source, ranges, proven, nested, depth + 1);
                }
                let start = block.position().byte_index();
                let sole = matches!(
                    block.next_including_whitespace_and_comments(),
                    Ok(Token::Ident(_) | Token::QuotedString(_))
                );
                let end = block.position().byte_index();
                if sole
                    && block.next_including_whitespace_and_comments().is_err()
                    && source[end..].starts_with(')')
                {
                    if let Some(i) = ranges.iter().position(|r| r.start == start && r.end == end) {
                        proven[i] = true;
                    }
                }
                // Any remaining tokens (and nested blocks) end with this block.
                Ok(())
            })?;
        }
    }
    let mut proven = vec![false; ranges.len()];
    let mut input = ParserInput::new(source);
    let mut input = cssparser::Parser::new(&mut input);
    walk(&mut input, source, ranges, &mut proven, true, 0).is_ok() && proven.iter().all(|p| *p)
}

/// Attribute names AngleSharp compares with `OrdinalIgnoreCase` when no flag is
/// given (`DefaultAttributeSelectorFactory.cs:14-62,139-144`). The lookup is an
/// ordinal `HashSet` over the name as written, so `[TYPE!=x]` stays ordinal.
const CASE_INSENSITIVE_ATTRIBUTES: [&str; 46] = [
    "accept",
    "accept-charset",
    "align",
    "alink",
    "axis",
    "bgcolor",
    "charset",
    "checked",
    "clear",
    "codetype",
    "color",
    "compact",
    "declare",
    "defer",
    "dir",
    "direction",
    "disabled",
    "enctype",
    "face",
    "frame",
    "hreflang",
    "http-equiv",
    "lang",
    "language",
    "link",
    "media",
    "method",
    "multiple",
    "nohref",
    "noresize",
    "noshade",
    "nowrap",
    "readonly",
    "rel",
    "rev",
    "rules",
    "scope",
    "scrolling",
    "selected",
    "shape",
    "target",
    "text",
    "type",
    "valign",
    "valuetype",
    "vlink",
];
/// Maximum nested blocks walked when the source contains `!`. Unlike the
/// selector parenthesis limit, this also counts brackets and braces, so some
/// otherwise-valid selectors at the boundary are conservatively rejected.
const MAX_REWRITE_DEPTH: usize = 32;

/// Rewrites AngleSharp's `[name!=value]` into `:not([name="value" s])`.
///
/// Upstream tokenizes `!=` only when adjacent (`CssTokenizer.cs:314-323`) and
/// matches `!String.Equals(GetAttribute(name), value, Ordinal)`
/// (`AttrNotMatchSelector.cs:21`), so a missing attribute matches. Only that
/// ordinal case is translated: a plain ident name, an ident, string or digit
/// value and an optional `s` flag, in a selector context (top level or
/// `:not`/`:is`/`:where`/`:has`). Every other `!` outside strings and comments
/// is an error here, before scraper's forgiving `:is`/`:where` lists can drop
/// it. Rejected: the `i` flag and the names above without `s` (.NET
/// `OrdinalIgnoreCase` is not ASCII folding), namespaces and `*`, names with
/// `:` or `xmlns` (upstream `GetAttribute` matches qualified names, which
/// differ from no-namespace local names on adjusted foreign attributes).
fn rewrite_inequality(source: &str) -> Result<Cow<'_, str>, String> {
    if !source.contains('!') {
        return Ok(Cow::Borrowed(source));
    }
    let mut input = ParserInput::new(source);
    let mut input = cssparser::Parser::new(&mut input);
    let mut rewriter = Rewriter {
        source,
        out: String::new(),
        copied: 0,
    };
    input
        .parse_entirely(|input| rewriter.walk(input, true, 0))
        .map_err(|e| match e.kind {
            cssparser::ParseErrorKind::Custom(reason) => reason,
            cssparser::ParseErrorKind::Basic(_) => "malformed selector",
        })?;
    if rewriter.copied == 0 {
        return Ok(Cow::Borrowed(source));
    }
    rewriter.out.push_str(&source[rewriter.copied..]);
    Ok(Cow::Owned(rewriter.out))
}

struct Rewriter<'s> {
    source: &'s str,
    out: String,
    /// Source bytes already copied to `out`.
    copied: usize,
}
type RewriteError<'i> = cssparser::ParseError<'i, &'static str>;
impl Rewriter<'_> {
    fn walk<'i>(
        &mut self,
        input: &mut cssparser::Parser<'i, '_>,
        selectors: bool,
        depth: usize,
    ) -> Result<(), RewriteError<'i>> {
        loop {
            let start = input.position();
            let token = match input.next_including_whitespace_and_comments() {
                Ok(token) => token.clone(),
                Err(_) => return Ok(()),
            };
            let nested = match token {
                Token::Delim('!') => {
                    return Err(input.new_custom_error("`!` outside an attribute selector"))
                }
                Token::SquareBracketBlock => None,
                Token::Function(ref name) => Some(
                    selectors
                        && ["not", "is", "where", "has"]
                            .iter()
                            .any(|f| name.eq_ignore_ascii_case(f)),
                ),
                Token::ParenthesisBlock | Token::CurlyBracketBlock => Some(false),
                _ => continue,
            };
            if depth >= MAX_REWRITE_DEPTH {
                return Err(input.new_custom_error("nesting limit exceeded"));
            }
            let Some(nested) = nested else {
                let mut content_end = start;
                let inequality = input.parse_nested_block(|block| {
                    let found = self.attribute(block, depth + 1);
                    content_end = block.position();
                    found
                })?;
                let Some((name, value)) = inequality else {
                    continue;
                };
                if !selectors {
                    return Err(input.new_custom_error("not in a selector list"));
                }
                if input.slice(content_end..input.position()) != "]" {
                    return Err(input.new_custom_error("unterminated attribute selector"));
                }
                self.out
                    .push_str(&self.source[self.copied..start.byte_index()]);
                self.out.push_str(":not([");
                cssparser::serialize_identifier(&name, &mut self.out)
                    .and_then(|()| {
                        self.out.push('=');
                        cssparser::serialize_string(&value, &mut self.out)
                    })
                    .expect("String writes are infallible");
                self.out.push_str(" s])");
                self.copied = input.position().byte_index();
                continue;
            };
            input.parse_nested_block(|block| self.walk(block, nested, depth + 1))?;
        }
    }

    /// Returns `(name, value)` for a supported `!=` attribute block, `None` for
    /// one without `!` at its own level (nested blocks are still walked).
    fn attribute<'i>(
        &mut self,
        input: &mut cssparser::Parser<'i, '_>,
        depth: usize,
    ) -> Result<Option<(String, String)>, RewriteError<'i>> {
        let mut tokens = Vec::new();
        loop {
            // Each token's own start, after whitespace and comments.
            let start = input.position();
            let token = match input.next_including_whitespace_and_comments() {
                Ok(Token::WhiteSpace(_) | Token::Comment(_)) => continue,
                Ok(token) => token.clone(),
                Err(_) => break,
            };
            if matches!(
                token,
                Token::Function(_)
                    | Token::ParenthesisBlock
                    | Token::SquareBracketBlock
                    | Token::CurlyBracketBlock
            ) {
                if depth >= MAX_REWRITE_DEPTH {
                    return Err(input.new_custom_error("nesting limit exceeded"));
                }
                input.parse_nested_block(|block| self.walk(block, false, depth + 1))?;
            }
            tokens.push((start, token, input.position()));
        }
        let Some(bang) = tokens.iter().position(|(_, t, _)| *t == Token::Delim('!')) else {
            return Ok(None);
        };
        let malformed = |reason| Err(input.new_custom_error(reason));
        let delim = |i: usize, c: &[char]| {
            tokens
                .get(i)
                .is_some_and(|(_, t, _)| matches!(t, Token::Delim(d) if c.contains(d)))
        };
        if delim(0, &['|', '*']) || delim(1, &['|']) {
            return malformed("namespace or wildcard attribute name unsupported");
        }
        if bang != 1 || (2..tokens.len()).any(|i| delim(i, &['!'])) {
            return malformed("malformed attribute inequality");
        }
        let name = match &tokens[0].1 {
            Token::Ident(name) => name.to_string(),
            _ => return malformed("attribute name must be an identifier"),
        };
        match tokens.get(2) {
            Some((start, Token::Delim('='), _)) if *start == tokens[1].2 => {}
            _ => return malformed("`!` must be immediately followed by `=`"),
        }
        let value = match tokens.get(3) {
            Some((_, Token::Ident(value), _)) => value.to_string(),
            Some((start, Token::QuotedString(value), end)) => {
                if !terminated_string(input.slice(*start..*end)) {
                    return malformed("unterminated string");
                }
                value.to_string()
            }
            // Upstream keeps a number's source text; only plain digits are
            // tokenized identically.
            Some((start, Token::Number { .. }, end)) => {
                let raw = input.slice(*start..*end);
                if !raw.bytes().all(|b| b.is_ascii_digit()) {
                    return malformed("value must be one identifier, string or digit sequence");
                }
                raw.to_owned()
            }
            _ => return malformed("value must be one identifier, string or digit sequence"),
        };
        let sensitive = match tokens.get(4) {
            None => false,
            Some((_, Token::Ident(flag), _)) if flag.eq_ignore_ascii_case("s") => true,
            Some((_, Token::Ident(flag), _)) if flag.eq_ignore_ascii_case("i") => {
                return malformed("`i` flag unsupported (.NET OrdinalIgnoreCase)")
            }
            Some(_) => return malformed("malformed attribute inequality"),
        };
        if tokens.len() > 5 {
            return malformed("malformed attribute inequality");
        }
        // AngleSharp starts no identifier with `--` (`CssTokenizer.cs:113-143`)
        // and appends a newline for an escaped one (`CssTokenizer.cs:368-374`);
        // NUL, CR and FF handling is not verified against cssparser's.
        for i in [0, 3] {
            let (start, _, end) = &tokens[i];
            let raw = input.slice(*start..*end);
            if raw.starts_with("--") || raw.contains(['\0', '\r', '\x0C']) || raw.contains("\\\n") {
                return malformed("identifier or escape outside the verified tokenizer subset");
            }
        }
        if name.contains(':') || name.eq_ignore_ascii_case("xmlns") {
            return malformed("qualified attribute names unsupported");
        }
        if !sensitive && CASE_INSENSITIVE_ATTRIBUTES.contains(&name.as_str()) {
            return malformed(
                "HTML case-insensitive attribute without `s` unsupported (.NET OrdinalIgnoreCase)",
            );
        }
        Ok(Some((name, value)))
    }
}

/// Compiled selector list. Matching order and semantics are scraper's.
#[derive(Clone, Debug)]
pub struct Css {
    selectors: SelectorList<Impl>,
    scans_text: bool,
}
impl Css {
    /// A positive check of one element's own name, id, classes and attributes.
    /// No ancestor/sibling/text/pseudo-class evaluation can depend on a select
    /// subtree that the two HTML parsers build differently. Inspect the parsed
    /// components, never punctuation inside a selector string or attribute value.
    pub(super) fn is_local_presence(&self) -> bool {
        use parser::Component;
        self.selectors.slice().iter().all(|selector| {
            selector.iter_raw_match_order().all(|component| {
                matches!(
                    component,
                    Component::LocalName(_)
                        | Component::ID(_)
                        | Component::Class(_)
                        | Component::AttributeInNoNamespaceExists { .. }
                        | Component::AttributeInNoNamespace { .. }
                        | Component::ExplicitUniversalType
                )
            })
        })
    }
    pub fn parse(source: &str) -> Result<Self, String> {
        let rewritten = rewrite_inequality(source).map_err(|reason| {
            format!("unsupported CSS selector {source:?}: invalid [attribute!=value]: {reason}")
        })?;
        if let Cow::Owned(rewritten) = &rewritten {
            super::selector::check_css_bounds(rewritten)
                .map_err(|e| format!("{e} after [attribute!=value] rewriting"))?;
        }
        let parser = Parser {
            source: &rewritten,
            contains_error: Cell::new(None),
            scans_text: Cell::new(false),
        };
        let mut input = ParserInput::new(&rewritten);
        let mut input = cssparser::Parser::new(&mut input);
        let parsed = SelectorList::parse(&parser, &mut input, ParseRelative::No);
        // Forgiving :is()/:where() lists must not hide an invalid :contains.
        if let Some(reason) = parser.contains_error.get() {
            return Err(format!(
                "unsupported CSS selector {source:?}: invalid :contains argument: {reason}"
            ));
        }
        let scans_text = parser.scans_text.get();
        parsed
            .map(|selectors| Self {
                selectors,
                scans_text,
            })
            .map_err(|e| {
                format!(
                    "unsupported CSS selector {source:?}: {:?}",
                    SelectorErrorKind::from(e)
                )
            })
    }
    /// Same matching context as `scraper::Selector::matches`.
    ///
    /// A selector with a non-empty `:contains` fails on a marked document before
    /// matching, even where matching would never reach that pseudo-class.
    pub fn matches(&self, element: &ElementRef<'_>, scan: &Scan) -> Result<bool, String> {
        if self.scans_text && is_marked(element) {
            scan.fail(DROPPED_SHADOW_ROOT);
        }
        let mut caches = matching::SelectorCaches::default();
        let mut context = MatchingContext::new(
            matching::MatchingMode::Normal,
            None,
            &mut caches,
            matching::QuirksMode::NoQuirks,
            matching::NeedsSelectorFlags::No,
            matching::MatchingForInvalidation::No,
        );
        let node = Node {
            element: *element,
            scan,
            xml: super::xml::is_xml(element),
        };
        let matched = scan.failure.get().is_none()
            && self
                .selectors
                .slice()
                .iter()
                .any(|s| matching::matches_selector(s, 0, None, &node, &mut context));
        if let Some(reason) = scan.failure.get() {
            return Err(reason.into());
        }
        Ok(matched)
    }
}

/// `ElementRef` carrying the scan budget and XML matching mode. HTML features
/// other than `:contains` delegate to scraper's implementation.
#[derive(Clone, Debug)]
struct Node<'a, 's> {
    element: ElementRef<'a>,
    scan: &'s Scan,
    xml: bool,
}
impl<'a, 's> Node<'a, 's> {
    fn wrap(&self, element: Option<ElementRef<'a>>) -> Option<Self> {
        element.map(|element| Self {
            element,
            scan: self.scan,
            xml: self.xml,
        })
    }
}
impl selectors::Element for Node<'_, '_> {
    type Impl = Impl;
    fn opaque(&self) -> OpaqueElement {
        self.element.opaque()
    }
    fn parent_element(&self) -> Option<Self> {
        self.wrap(self.element.parent_element())
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        self.element.parent_node_is_shadow_root()
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        self.wrap(self.element.containing_shadow_host())
    }
    fn is_pseudo_element(&self) -> bool {
        self.element.is_pseudo_element()
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.wrap(self.element.prev_sibling_element())
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.wrap(self.element.next_sibling_element())
    }
    fn first_element_child(&self) -> Option<Self> {
        self.wrap(self.element.first_element_child())
    }
    fn is_html_element_in_html_document(&self) -> bool {
        !self.xml && self.element.is_html_element_in_html_document()
    }
    fn has_local_name(&self, name: &CssLocalName) -> bool {
        if self.xml {
            self.element
                .value()
                .name
                .local
                .as_ref()
                .eq_ignore_ascii_case(name.0.as_ref())
        } else {
            self.element.has_local_name(name)
        }
    }
    fn has_namespace(&self, ns: &<Impl as SelectorImpl>::BorrowedNamespaceUrl) -> bool {
        self.element.has_namespace(ns)
    }
    fn is_same_type(&self, other: &Self) -> bool {
        self.element.is_same_type(&other.element)
    }
    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&<Impl as SelectorImpl>::NamespaceUrl>,
        local_name: &CssLocalName,
        operation: &AttrSelectorOperation<&CssString>,
    ) -> bool {
        self.element.attr_matches(ns, local_name, operation)
    }
    fn match_non_ts_pseudo_class(
        &self,
        contains: &Contains,
        _: &mut MatchingContext<'_, Impl>,
    ) -> bool {
        contains.matches(self.element, self.scan)
    }
    fn match_pseudo_element(
        &self,
        pseudo: &PseudoElement,
        _: &mut MatchingContext<'_, Impl>,
    ) -> bool {
        match *pseudo {}
    }
    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        self.element.apply_selector_flags(flags)
    }
    fn is_link(&self) -> bool {
        self.element.is_link()
    }
    fn is_html_slot_element(&self) -> bool {
        self.element.is_html_slot_element()
    }
    fn has_id(&self, id: &CssLocalName, case_sensitivity: CaseSensitivity) -> bool {
        self.element.has_id(id, case_sensitivity)
    }
    fn has_class(&self, name: &CssLocalName, case_sensitivity: CaseSensitivity) -> bool {
        self.element.has_class(name, case_sensitivity)
    }
    fn has_custom_state(&self, name: &CssLocalName) -> bool {
        self.element.has_custom_state(name)
    }
    fn imported_part(&self, name: &CssLocalName) -> Option<CssLocalName> {
        self.element.imported_part(name)
    }
    fn is_part(&self, name: &CssLocalName) -> bool {
        self.element.is_part(name)
    }
    fn is_empty(&self) -> bool {
        if self.xml {
            !self.element.children().any(|node| {
                node.value().is_element()
                    || super::xml::text(node.value()).is_some_and(|text| !text.is_empty())
            })
        } else {
            self.element.is_empty()
        }
    }
    fn is_root(&self) -> bool {
        self.element.is_root()
    }
    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        self.element.add_element_unique_hashes(filter)
    }
}

/// `Html::parse_document` with the same scraper sink, but with scripting disabled
/// like Jackett's `new HtmlParser()` (`HtmlParser.cs:64-67,83`): HTML `noscript`
/// content is parsed as markup in body (`HtmlDomBuilder.cs:1372-1385`) and
/// relocated from head to body when it is not head content
/// (`HtmlDomBuilder.cs:797-812,912-970`), as html5ever does without scripting.
///
/// The sink records one document whose text cannot match AngleSharp, so
/// `:contains` fails explicitly on it: html5ever 0.29 drops a declarative shadow
/// root template and inserts its contents into the host as ordinary children,
/// while AngleSharp keeps them in a shadow root outside `ChildNodes`
/// (`HtmlTemplateElement.cs:60-82`). The marker is a processing instruction
/// before the root element, invisible to selectors and text extraction; the HTML
/// parser never creates one.
pub fn parse_document(document: &str) -> Html {
    parse_document_guarded(document, None)
}
/// A select token that html5ever drops can change parsing beyond the select.
/// For example AngleSharp enters raw text on <xmp> in select while html5ever
/// ignores it (HtmlDomBuilder.cs:2390; html5ever rules.rs:1197). Reject unexpected
/// tokens in either select mode before trusting even a later local login marker.
/// Only this authentication parse requests exact errors; raw diagnostic text is
/// never retained or exposed. The message suffix is pinned to html5ever 0.29.1
/// tree_builder/mod.rs:611-618 and covered by malformed-select fixtures.
pub(super) fn parse_presence_document(document: &str) -> Result<Html, String> {
    let invalid_select = Cell::new(false);
    let html = parse_document_guarded(document, Some(&invalid_select));
    if invalid_select.get() {
        return Err("HTML authentication document has unsupported select markup".into());
    }
    Ok(html)
}
fn parse_document_guarded(document: &str, select_errors: Option<&Cell<bool>>) -> Html {
    let opts = ParseOpts {
        tree_builder: TreeBuilderOpts {
            scripting_enabled: false,
            exact_errors: select_errors.is_some(),
            ..Default::default()
        },
        ..Default::default()
    };
    html5ever::driver::parse_document(
        ShadowRootSink {
            inner: HtmlTreeSink::new(Html::new_document()),
            dropped: Cell::new(false),
            select_errors,
        },
        opts,
    )
    .one(document)
}

/// Whether [`parse_document`] marked the element's document for a dropped
/// declarative shadow root. Markers are leading root children.
fn is_marked(element: &ElementRef<'_>) -> bool {
    element
        .tree()
        .root()
        .children()
        .map_while(|n| n.value().as_processing_instruction())
        .any(|pi| *pi.target == *DROPPED_SHADOW_ROOT_MARKER)
}

const XHTML: &str = "http://www.w3.org/1999/xhtml";
const ERROR_SHADOW_ROOT: &str =
    "HTML declarative shadow root template unsupported for search error checks";
const ERROR_SELECT: &str = "HTML select element unsupported for search error checks";

/// Fail-closed guard for `search.error` documents, whatever the selector (even one
/// matching the root first or without `:contains`). A missed error block would
/// otherwise become a silent empty or wrong success; ordinary selection is unchanged.
///
/// - The [`parse_document`] marker for a dropped declarative shadow root (html5ever
///   creates no element for it, `tree_builder/rules.rs:156-170`).
/// - Any HTML `template` with a `shadowrootmode` attribute (any value; the tokenizer
///   lowercases the name). html5ever keeps e.g. `closed` or `OPEN` as an element that
///   AngleSharp removes or turns into a shadow root (`HtmlTemplateElement.cs:60-82`).
/// - Any HTML `select`: html5ever's in-select rules drop markup such as `span` that
///   AngleSharp's select parsing keeps (`HtmlDomBuilder.cs:2314-2400,2465-2520`).
///   Every select subtree parse needs a select element, so its presence is the signal.
///
/// The scan covers every arena node, detached ones and inert template contents
/// included, so it is conservative. Foreign (SVG/MathML) elements are ordinary.
/// Callers bound the arena first (`selector::check_document`).
pub fn check_error_document(html: &Html) -> Result<(), String> {
    check_document_markers(html, false)
}
/// Login presence may ignore a select subtree only for positive local selectors.
/// Shadow-root documents still fail as a whole. The returned flag requires the
/// caller to exclude HTML select subtrees from matching, not just skip the guard.
pub(super) fn check_presence_document(html: &Html, css: &Css) -> Result<bool, String> {
    let local = css.is_local_presence();
    check_document_markers(html, local)?;
    Ok(local)
}
fn check_document_markers(html: &Html, allow_select: bool) -> Result<(), String> {
    if is_marked(&html.root_element()) {
        return Err(ERROR_SHADOW_ROOT.into());
    }
    for node in html.tree.values() {
        let Some(e) = node.as_element() else {
            continue;
        };
        if &*e.name.ns != XHTML {
            continue;
        }
        match &*e.name.local {
            "select" if !allow_select => return Err(ERROR_SELECT.into()),
            "template" if e.attrs().any(|(name, _)| name == "shadowrootmode") => {
                return Err(ERROR_SHADOW_ROOT.into())
            }
            _ => {}
        }
    }
    Ok(())
}

struct ShadowRootSink<'a> {
    inner: HtmlTreeSink,
    dropped: Cell<bool>,
    select_errors: Option<&'a Cell<bool>>,
}
impl TreeSink for ShadowRootSink<'_> {
    type Handle = <HtmlTreeSink as TreeSink>::Handle;
    type Output = Html;
    type ElemName<'a>
        = <HtmlTreeSink as TreeSink>::ElemName<'a>
    where
        Self: 'a;
    fn finish(self) -> Html {
        let mut html = self.inner.finish();
        if self.dropped.get() {
            html.tree
                .root_mut()
                .prepend(scraper::Node::ProcessingInstruction(
                    ProcessingInstruction {
                        target: DROPPED_SHADOW_ROOT_MARKER.into(),
                        data: "".into(),
                    },
                ));
        }
        html
    }
    fn parse_error(&self, msg: Cow<'static, str>) {
        if let Some(invalid) = self.select_errors {
            if msg.starts_with("Unexpected token ")
                && (msg.ends_with(" in insertion mode InSelect")
                    || msg.ends_with(" in insertion mode InSelectInTable"))
            {
                invalid.set(true);
            }
            return;
        }
        self.inner.parse_error(msg)
    }
    fn get_document(&self) -> Self::Handle {
        self.inner.get_document()
    }
    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        self.inner.elem_name(target)
    }
    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Self::Handle {
        self.inner.create_element(name, attrs, flags)
    }
    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        self.inner.create_comment(text)
    }
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        self.inner.create_pi(target, data)
    }
    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        self.inner.append(parent, child)
    }
    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        self.inner
            .append_based_on_parent_node(element, prev_element, child)
    }
    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        self.inner
            .append_doctype_to_document(name, public_id, system_id)
    }
    fn mark_script_already_started(&self, node: &Self::Handle) {
        self.inner.mark_script_already_started(node)
    }
    fn pop(&self, node: &Self::Handle) {
        self.inner.pop(node)
    }
    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        self.inner.get_template_contents(target)
    }
    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        self.inner.same_node(x, y)
    }
    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.inner.set_quirks_mode(mode)
    }
    fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
        self.inner.append_before_sibling(sibling, new_node)
    }
    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        self.inner.add_attrs_if_missing(target, attrs)
    }
    fn associate_with_form(
        &self,
        target: &Self::Handle,
        form: &Self::Handle,
        nodes: (&Self::Handle, Option<&Self::Handle>),
    ) {
        self.inner.associate_with_form(target, form, nodes)
    }
    fn remove_from_parent(&self, target: &Self::Handle) {
        self.inner.remove_from_parent(target)
    }
    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        self.inner.reparent_children(node, new_parent)
    }
    fn is_mathml_annotation_xml_integration_point(&self, handle: &Self::Handle) -> bool {
        self.inner
            .is_mathml_annotation_xml_integration_point(handle)
    }
    fn set_current_line(&self, line_number: u64) {
        self.inner.set_current_line(line_number)
    }
    fn complete_script(&self, node: &Self::Handle) -> NextParserState {
        self.inner.complete_script(node)
    }
    fn allow_declarative_shadow_roots(&self, intended_parent: &Self::Handle) -> bool {
        self.inner.allow_declarative_shadow_roots(intended_parent)
    }
    fn attach_declarative_shadow(
        &self,
        location: &Self::Handle,
        attrs: Vec<Attribute>,
    ) -> Result<(), String> {
        let attached = self.inner.attach_declarative_shadow(location, attrs);
        if attached.is_err() {
            self.dropped.set(true);
        }
        attached
    }
}
