//! Definition-sourced .NET regex syntax normalized to Rust `regex` syntax.
//!
//! Source: dotnet/runtime v9.0.0 9d5a6a9, RegexParser.cs. Jackett constructs definition
//! regexes with no options (CardigannIndexer.cs:345/1143/1151). One bounded, iterative
//! scanner reads the original .NET tokens; generated Rust syntax (nested block classes,
//! canonical classes) is only ever appended to the output and never rescanned. Constructs
//! whose .NET and Rust readings diverge are rewritten only when the .NET reading is exact
//! from source, and otherwise rejected here. Other constructs Rust does not support
//! (look-around, `\1` backreferences, octal, ...) are left for Rust to reject; constructs the
//! scanner rejects first (e.g. `(?#..)`, `\'name'` references) get this module's message.
//! Every non-BMP pattern character is rejected: a deliberately conservative scope limit,
//! since .NET matches UTF-16 code units and Rust matches scalar values.
use std::borrow::Cow;
use std::fmt::Write as _;

/// Source: RegexCharClass.cs:203/212 (s_propTable start and exclusive end; ordinal lookup at
/// :1828). Rust's loose matching instead strips the `Is` prefix and resolves e.g.
/// `IsCyrillic` to the Cyrillic *script*.
const DOTNET_BLOCKS: [(&str, &str); 2] = [
    ("IsCJKUnifiedIdeographs", r"\x{4E00}-\x{9FFF}"),
    ("IsCyrillic", r"\x{0400}-\x{04FF}"),
];
/// Source: RegexCharClass.cs:116-171 (s_definedCategories, ordinal keys). These keep
/// Rust's general-category meaning. Other .NET blocks remain unsupported; Rust's loose
/// property aliases must not silently turn those names into scripts or other properties.
pub(super) const DOTNET_CATEGORIES: [&str; 37] = [
    "Cc", "Cf", "Cn", "Co", "Cs", "C", "Ll", "Lm", "Lo", "Lt", "Lu", "L", "Mc", "Me", "Mn", "M",
    "Nd", "Nl", "No", "N", "Pc", "Pd", "Pe", "Po", "Ps", "Pf", "Pi", "P", "Sc", "Sk", "Sm", "So",
    "S", "Zl", "Zp", "Zs", "Z",
];

fn err<T>(what: &str) -> Result<T, String> {
    Err(format!("unsupported/invalid .NET regex: {what}"))
}

/// One escape sequence, read as .NET does (ScanBackslash/ScanCharEscape :1076-1590).
enum Escape {
    /// A single literal character (`\x41`, `\.`, `\-`, `\t`).
    Char(char),
    /// `\d \D \s \S \w \W` or a supported `\p{Category}`; copied verbatim.
    Set,
    /// `\p{Is<Block>}`/`\P{Is<Block>}` as an explicit BMP interval.
    Block(&'static str, bool),
    /// Anything else (anchors, backreferences, octal, `\c`, `\e`, ...): copied verbatim
    /// outside classes so Rust keeps rejecting what it does not support.
    Verbatim,
}

/// Read the escape at `chars[i] == '\\'`, returning it and the index after it.
fn escape(chars: &[char], i: usize, in_class: bool) -> Result<(Escape, usize), String> {
    let Some(&kind) = chars.get(i + 1) else {
        return if in_class {
            err("unclosed character class")
        } else {
            Ok((Escape::Verbatim, i + 1))
        };
    };
    let hex = |len: usize| -> Option<u32> {
        let digits = chars.get(i + 2..i + 2 + len)?;
        digits
            .iter()
            .try_fold(0, |v, c| Some(v * 16 + c.to_digit(16)?))
    };
    let escape = match kind {
        'p' | 'P' => {
            // ParseProperty :1618 needs `{Name}`; Rust's `\pL` single-letter form is not .NET.
            if chars.get(i + 2) != Some(&'{') {
                return err("\\p requires a {Name} property");
            }
            let Some(len) = chars[i + 3..].iter().position(|&c| c == '}') else {
                return err("unterminated Unicode property escape");
            };
            let name = chars[i + 3..i + 3 + len].iter().collect::<String>();
            let end = i + 4 + len;
            return if DOTNET_CATEGORIES.contains(&name.as_str()) {
                Ok((Escape::Set, end))
            } else if let Some((_, range)) = DOTNET_BLOCKS.iter().find(|(n, _)| *n == name) {
                Ok((Escape::Block(range, kind == 'P'), end))
            } else {
                err(&format!("unrecognized Unicode property {name:?}"))
            };
        }
        'd' | 'D' | 's' | 'S' | 'w' | 'W' => Escape::Set,
        // ScanHex :1478 reads exactly 2/4 digits; `\x{..}`/`\u{..}` are Rust-only.
        'x' | 'u' => {
            let len = if kind == 'x' { 2 } else { 4 };
            if chars.get(i + 2) == Some(&'{') {
                return err("braced hex escape");
            }
            let Some(value) = hex(len) else {
                return err("hex escape needs exactly 2 (\\x) or 4 (\\u) digits");
            };
            let Some(c) = char::from_u32(value) else {
                return err("UTF-16 surrogate escape");
            };
            return Ok((Escape::Char(c), i + 2 + len));
        }
        // Rust reads these as `\U` hex, `_` literal, word-boundary assertions and special
        // boundaries; .NET rejects `\U`/`\_` and reads `\<`/`\>`/`\b{` differently.
        'U' | '_' | '<' | '>' => return err(&format!("escape \\{kind}")),
        'b' | 'B' if !in_class && chars.get(i + 2) == Some(&'{') => {
            return err("\\b or \\B followed by '{'")
        }
        'a' => Escape::Char('\u{7}'),
        'f' => Escape::Char('\u{c}'),
        'n' => Escape::Char('\n'),
        'r' => Escape::Char('\r'),
        't' => Escape::Char('\t'),
        'v' => Escape::Char('\u{b}'),
        // ScanBasicBackslash :1185-1272 reads `\'number'`/`\'name'` outside classes as a
        // (deprecated) backreference, or throws when it is undefined; never as literals.
        '\'' if !in_class && apostrophe_reference(chars, i + 2) => {
            return err("\\'number' or \\'name' backreference");
        }
        // Non-word ASCII is literal in .NET (ScanCharEscape default) and escapable in Rust.
        c if c.is_ascii() && !c.is_ascii_alphanumeric() => Escape::Char(c),
        _ if in_class => return err(&format!("escape \\{kind} in a character class")),
        _ => Escape::Verbatim,
    };
    Ok((escape, i + 2))
}

/// Whether `\'` followed by `chars[from..]` is a .NET reference rather than a literal quote.
/// Numbers: ScanDecimal :1457 (ASCII digits, throws past `int.MaxValue`) then the closing
/// quote. Names: IsBoundaryWordChar (RegexCharClass.cs:1231) is exactly `[0-9A-Za-z_]` for
/// ASCII but depends on .NET's Unicode tables otherwise, so any non-ASCII character is assumed
/// to continue the name (over-rejecting, never accepting a reference as a literal).
fn apostrophe_reference(chars: &[char], from: usize) -> bool {
    let rest = &chars[from.min(chars.len())..];
    let Some(&first) = rest.first() else {
        return false;
    };
    let len = if first.is_ascii_digit() {
        let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
        let value = rest[..digits].iter().try_fold(0i32, |v, c| {
            v.checked_mul(10)?.checked_add(c.to_digit(10)? as i32)
        });
        if value.is_none() {
            return true;
        }
        digits
    } else if first.is_ascii_alphabetic() || first == '_' || !first.is_ascii() {
        rest.iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_' || !c.is_ascii())
            .count()
    } else {
        return false;
    };
    rest.get(len) == Some(&'\'')
}

/// IsTrueQuantifier :1974: `{` followed by ASCII `digits}`, `digits,}` or `digits,digits}`.
/// Returns the index of the closing brace.
fn true_quantifier(chars: &[char], i: usize) -> Option<usize> {
    let digits = |from: usize| {
        chars[from..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    let mut j = i + 1;
    let n = digits(j);
    if n == 0 {
        return None;
    }
    j += n;
    if chars.get(j) == Some(&',') {
        j += 1;
        j += digits(j);
    }
    (chars.get(j) == Some(&'}')).then_some(j)
}

fn push_block(out: &mut String, range: &str, negated: bool) {
    out.push('[');
    if negated {
        out.push('^');
    }
    out.push_str(range);
    out.push(']');
}

/// Class items as .NET reads them, in source order.
enum Item {
    Char(char),
    Range(char, char),
    /// Verbatim `\d`-style shorthand or `\p{Category}` text.
    Set(String),
    Block(&'static str, bool),
}

fn push_class_char(out: &mut String, c: char) {
    if c.is_ascii_alphanumeric() {
        out.push(c);
    } else {
        let _ = write!(out, "\\x{{{:X}}}", c as u32);
    }
}

struct Class {
    /// Original text with only block properties rewritten.
    raw: String,
    items: Vec<Item>,
    negated: bool,
    /// Why Rust would read the original text differently, if it would.
    hazard: Option<&'static str>,
    /// Block property adjacent to a dash (kept rejected as before).
    block_dash: bool,
    has_block: bool,
}

/// ScanCharClass :547-741 from just after `[`. Subtraction, POSIX-like `[:`, shorthand range
/// endpoints and reversed or unclosed ranges are rejected; .NET has no other nesting.
fn class(chars: &[char], mut i: usize) -> Result<(Class, usize), String> {
    let mut c = Class {
        raw: String::from("["),
        items: Vec::new(),
        negated: chars.get(i) == Some(&'^'),
        hazard: None,
        block_dash: false,
        has_block: false,
    };
    if c.negated {
        c.raw.push('^');
        i += 1;
    }
    let (mut first, mut range_start, mut prev_dash) = (true, None::<char>, false);
    loop {
        let Some(&ch) = chars.get(i) else {
            return err("unclosed character class");
        };
        if ch > '\u{FFFF}' {
            return err("non-BMP character (UTF-16 semantics are not modelled)");
        }
        if ch == ']' && !first {
            c.raw.push(']');
            return Ok((c, i + 1));
        }
        let start = i;
        let (atom, escaped) = if ch == '\\' {
            let (atom, end) = escape(chars, i, true)?;
            i = end;
            (atom, true)
        } else {
            i += 1;
            (Escape::Char(ch), false)
        };
        // Only a literal `\-` (not `\x2D`) skips range detection (:640).
        let escaped_dash = escaped && chars[start + 1] == '-';
        let skips_range = escaped_dash || matches!(atom, Escape::Set | Escape::Block(..));
        let next = chars.get(i).copied();
        let after = chars.get(i + 1).copied();
        match &atom {
            Escape::Block(range, negated) => {
                push_block(&mut c.raw, range, *negated);
                c.has_block = true;
                c.block_dash |= prev_dash || next == Some('-');
            }
            _ => c.raw.extend(&chars[start..i]),
        }
        if !escaped {
            if ch == '[' {
                if next == Some(':') {
                    return err("'[:' in a character class");
                }
                c.hazard = Some("unescaped '[' in a character class");
            }
            // Rust class set operators.
            if matches!(ch, '&' | '~' | '-') && next == Some(ch) {
                c.hazard = Some("doubled '&', '~' or '-' in a character class");
            }
        }
        prev_dash = ch == '-' && !escaped && !first;
        if let Some(lo) = range_start.take() {
            let hi = match atom {
                Escape::Char('[') if !escaped => return err("character class subtraction"),
                Escape::Char(hi) => hi,
                _ => return err("shorthand class in a character range"),
            };
            if lo > hi {
                return err("reversed character range");
            }
            c.items.push(Item::Range(lo, hi));
            first = false;
            continue;
        }
        let single = match atom {
            Escape::Set => Item::Set(chars[start..i].iter().collect()),
            Escape::Block(range, negated) => Item::Block(range, negated),
            Escape::Verbatim => unreachable!("class escapes are classified"),
            Escape::Char('-') if escaped_dash => Item::Char('-'),
            Escape::Char(lo) if next == Some('-') && after.is_some_and(|a| a != ']') => {
                range_start = Some(lo);
                c.raw.push('-');
                i += 1;
                if after == Some('-') {
                    c.hazard = Some("doubled '-' in a character class");
                }
                if first && lo == ']' && !escaped {
                    c.hazard = Some("leading ']' starting a range");
                }
                first = false;
                continue;
            }
            Escape::Char('-') if !escaped && next == Some('[') && !first => {
                return err("character class subtraction");
            }
            Escape::Char(ch) => Item::Char(ch),
        };
        // Shorthands and `\-` `continue` past range detection (:586-659), so a following dash
        // is literal in .NET but a range (or an error) in Rust.
        if skips_range && next == Some('-') && after.is_some_and(|a| a != ']') {
            c.hazard = Some("dash after a shorthand or escaped dash");
        }
        c.items.push(single);
        first = false;
    }
}

fn push_canonical(out: &mut String, class: &Class) {
    out.push('[');
    if class.negated {
        out.push('^');
    }
    for item in &class.items {
        match item {
            Item::Char(c) => push_class_char(out, *c),
            Item::Range(lo, hi) => {
                push_class_char(out, *lo);
                out.push('-');
                push_class_char(out, *hi);
            }
            Item::Set(text) => out.push_str(text),
            Item::Block(range, negated) => push_block(out, range, *negated),
        }
    }
    out.push(']');
}

/// Normalize a definition regex. Returns the input unchanged unless it uses a .NET block
/// property, a literal `{` (not IsTrueQuantifier), or a character class Rust would read
/// differently. Block properties stay rejected next to any other rewrite, inline options,
/// comments or look-around, as before.
pub(super) fn normalize(pattern: &str) -> Result<Cow<'_, str>, String> {
    #[cfg(test)]
    tests::SEEN.with(|seen| seen.borrow_mut().push(pattern.to_owned()));
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(pattern.len());
    let (mut rewritten, mut has_block, mut gate) = (false, false, None::<&str>);
    // 0: no quantifier, 1: after a quantifier, 2: after a lazy quantifier.
    let mut quantified = 0u8;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c > '\u{FFFF}' {
            return err("non-BMP character (UTF-16 semantics are not modelled)");
        }
        let mut quantifier = false;
        match c {
            '\\' => {
                let (escape, end) = escape(&chars, i, false)?;
                if let Escape::Block(range, negated) = escape {
                    push_block(&mut out, range, negated);
                    has_block = true;
                } else {
                    out.extend(&chars[i..end]);
                }
                i = end;
            }
            '[' => {
                let (class, end) = class(&chars, i + 1)?;
                has_block |= class.has_block;
                if class.block_dash {
                    gate = Some("character range adjacent to a block property");
                }
                if class.hazard.is_some() {
                    push_canonical(&mut out, &class);
                    rewritten = true;
                } else {
                    out.push_str(&class.raw);
                }
                i = end;
            }
            '(' if chars.get(i + 1) == Some(&'?') => {
                let rest = &chars[i + 2..];
                // ScanBlank :1057 skips `(?#...)` even without IgnorePatternWhitespace.
                if rest.first() == Some(&'#') {
                    return err("inline comment group");
                }
                let flags = rest
                    .iter()
                    .take_while(|c| c.is_ascii_alphabetic() || matches!(c, '-' | '+'))
                    .collect::<Vec<_>>();
                if flags.iter().any(|c| !matches!(c, 'i' | 'm' | 's' | '-')) {
                    // `x` changes class/whitespace parsing; `n`, `+` and Rust's `U`/`R`/`u`/`P`
                    // differ or do not exist in .NET.
                    return err("unsupported inline option or group");
                }
                let plain = match rest {
                    [':', ..] => true,
                    ['<', n, ..] => n.is_alphanumeric() || *n == '_',
                    _ => false,
                };
                if !plain {
                    gate = Some("inline option, comment, or look-around group");
                }
                out.push_str("(?");
                i += 2;
            }
            '*' | '+' | '?' => {
                quantifier = true;
                out.push(c);
                i += 1;
            }
            '{' => {
                if let Some(end) = true_quantifier(&chars, i) {
                    quantifier = true;
                    out.extend(&chars[i..=end]);
                    i = end + 1;
                } else {
                    out.push_str(r"\{");
                    rewritten = true;
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
        // ScanRegex :410-427 throws on a quantifier after a quantifier (other than one lazy
        // `?`); Rust would instead nest them.
        quantified = match (quantifier, quantified, c) {
            (false, ..) => 0,
            (true, 1, '?') => 2,
            (true, 0, _) => 1,
            (true, ..) => return err("nested quantifier"),
        };
    }
    if has_block {
        if let Some(what) = gate.or(rewritten.then_some("literal normalization")) {
            return err(&format!("{what} with a .NET block property"));
        }
    }
    Ok(if rewritten || has_block {
        Cow::Owned(out)
    } else {
        debug_assert_eq!(out, pattern);
        Cow::Borrowed(pattern)
    })
}

#[cfg(test)]
mod tests {
    use super::super::filters::{bounded_regex, definition_regex, replace_regex, Filter};
    use super::super::template::{Context, Template};
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    thread_local! {
        pub(super) static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
    /// Pinned corpus regex (girotorrent/ilcorsaroblu/jpopsuki) and its 31 .NET members.
    const PUNCT: &str = r##"[\[!"#$%&'()*+,\-.\/:;<=>?@[\]^_`{|}~]"##;
    const PUNCT_SET: &str = r##"!"#$%&'()*+,-./:;<=>?@[]^_`{|}~"##;
    const PUNCT_OUT: &str = r"[\x{5B}\x{21}\x{22}\x{23}\x{24}\x{25}\x{26}\x{27}\x{28}\x{29}\x{2A}\x{2B}\x{2C}\x{2D}\x{2E}\x{2F}\x{3A}\x{3B}\x{3C}\x{3D}\x{3E}\x{3F}\x{40}\x{5B}\x{5D}\x{5E}\x{5F}\x{60}\x{7B}\x{7C}\x{7D}\x{7E}]";
    const PUNTO_FULL: &str = r"(?i)T[\s-_]?(\d{1,2})(.*)\[(\d{1,2})[x|\/|\\](\d{1,2})\]";
    const PUNTO_SHORT: &str = r"(?i)T[\s-_]?(\d{1,2})\b";
    const NEKO_TITLE: &str = "(.+?)( {Tags:.+)";
    const NEKO_TAGS: &str = ".+ {(Tags:.+?)}";
    /// Every corpus rewrite: (pattern, normalized, captures_len).
    const EXPECTED: [(&str, &str, usize); 5] = [
        (PUNCT, PUNCT_OUT, 1),
        (
            PUNTO_FULL,
            r"(?i)T[\s\x{2D}\x{5F}]?(\d{1,2})(.*)\[(\d{1,2})[x|\/|\\](\d{1,2})\]",
            5,
        ),
        (PUNTO_SHORT, r"(?i)T[\s\x{2D}\x{5F}]?(\d{1,2})\b", 2),
        (NEKO_TITLE, r"(.+?)( \{Tags:.+)", 3),
        (NEKO_TAGS, r".+ \{(Tags:.+?)}", 2),
    ];

    fn ctx() -> Context {
        Context {
            values: BTreeMap::new(),
            now: chrono::DateTime::from_timestamp(1700000000, 0).unwrap(),
        }
    }
    fn apply(args: &[&str], input: &str) -> String {
        let args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        Filter::compile("re_replace", &args)
            .unwrap()
            .apply(input, &ctx())
            .unwrap()
    }
    /// ASCII members of a whole-input match of `pattern`.
    fn ascii_members(pattern: &str) -> BTreeSet<char> {
        let re = definition_regex(&format!("^(?:{pattern})$")).unwrap();
        (0u8..0x80)
            .map(char::from)
            .filter(|c| re.is_match(&c.to_string()))
            .collect()
    }
    fn set(chars: impl IntoIterator<Item = char>) -> BTreeSet<char> {
        chars.into_iter().collect()
    }
    fn fold(s: &BTreeSet<char>) -> BTreeSet<char> {
        s.iter()
            .flat_map(|c| [c.to_ascii_lowercase(), c.to_ascii_uppercase()])
            .collect()
    }

    #[test]
    fn apostrophe_references_fail_in_every_definition_regex_entrypoint() {
        // ScanBasicBackslash chooses references before literal character escapes.
        // Rust alone accepts these as literal apostrophe text, so its parser is not a guard.
        let mut patterns = vec![
            r"(?<a>x)\'a'".to_owned(),
            r"x\'0'".to_owned(),
            r"\'zz'".to_owned(),
            r"(x)\'1'".to_owned(),
            r"\'01'".to_owned(),
            r"\'_name2'".to_owned(),
            r"\'2147483647'".to_owned(),
            r"\'2147483648'".to_owned(),
            r"\'2147483648".to_owned(),
        ];
        // Unicode names use .NET's category tables, including Mn/Pc and joiners.
        // We conservatively reject uncertain non-ASCII names rather than copy Rust's tables.
        for name in [
            "é", "中文", "a\u{301}", "\u{203f}", "\u{200c}", "\u{200d}", "١", "—",
        ] {
            patterns.push(format!("\\'{name}'"));
        }
        for p in patterns {
            let expected = "unsupported/invalid .NET regex: \\'number' or \\'name' backreference";
            assert_eq!(normalize(&p).unwrap_err(), expected, "{p}");
            assert_eq!(definition_regex(&p).unwrap_err(), expected, "{p}");
            assert_eq!(
                Filter::compile("regexp", std::slice::from_ref(&p)).unwrap_err(),
                expected
            );
            assert_eq!(
                Filter::compile("re_replace", &[p.clone(), "X".into()]).unwrap_err(),
                expected
            );
            let template = format!(r#"{{{{ re_replace .Query.Q "{p}" "X" }}}}"#);
            assert_eq!(Template::compile(&template).unwrap_err(), expected);
        }
        // Concrete controls for the bug: these are wrongly literal matches without the guard.
        assert!(bounded_regex(r"(?<a>x)\'a'").unwrap().is_match("x'a'"));
        assert!(bounded_regex(r"x\'0'").unwrap().is_match("x'0'"));
    }

    #[test]
    fn apostrophe_literal_fallback_and_class_quotes_are_preserved() {
        for (pattern, input) in [
            (r"\'(.+?)\'", "'value'"),
            (r"\'", "'"),
            (r"\''", "''"),
            (r"\'name", "'name"),
            (r"\'name-thing'", "'name-thing'"),
            (r"\'12x'", "'12x'"),
            (r"\'12", "'12"),
            (r"\'a b'", "'a b'"),
            (r"\\'name'", r"\'name'"),
            (r"[\'a']", "'"),
            (r"[\'0']", "0"),
        ] {
            assert_eq!(normalize(pattern).unwrap(), pattern);
            assert!(
                definition_regex(pattern).unwrap().is_match(input),
                "{pattern}"
            );
        }
        // Escaped quote remains literal even when another class member forces canonical output.
        assert!(definition_regex(r"[\'[]").unwrap().is_match("'"));
        assert_eq!(apply(&[r"\'(.+?)\'", "$1"], "'value'"), "value");
        let mut c = ctx();
        c.values
            .insert(".Query.Q".into(), serde_json::json!("'value'"));
        assert_eq!(
            Template::compile(r#"{{ re_replace .Query.Q "\'(.+?)\'" "$1" }}"#)
                .unwrap()
                .render(&c)
                .unwrap(),
            "value"
        );
    }

    #[test]
    fn pinned_torrentdd_literal_apostrophe_filter_still_executes() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("vendor/jackett/definitions/torrentdd.yml");
        let node = crate::cardigann::load(&file, crate::cardigann::Limits::default()).unwrap();
        let mut stack = vec![&node];
        let mut found = false;
        while let Some(n) = stack.pop() {
            match &n.value {
                crate::cardigann::Value::Mapping(pairs) => {
                    if n.string("name") == Some("regexp") {
                        if let Some(pattern) = n.get("args").and_then(|a| a.text()) {
                            if pattern == r"\'(.+?)\'" {
                                found = true;
                                assert_eq!(
                                    Filter::compile("regexp", &[pattern.into()])
                                        .unwrap()
                                        .apply("call('fixture-value')", &ctx())
                                        .unwrap(),
                                    "fixture-value"
                                );
                            }
                        }
                    }
                    stack.extend(pairs.iter().map(|(_, v)| v));
                }
                crate::cardigann::Value::Sequence(v) => stack.extend(v),
                _ => {}
            }
        }
        assert!(
            found,
            "unchanged pinned torrentdd quote filter must be exercised"
        );
    }

    #[test]
    fn punctuation_class_is_the_31_dotnet_literals() {
        assert_eq!(normalize(PUNCT).unwrap(), PUNCT_OUT);
        assert_eq!(PUNCT_SET.len(), 31);
        assert_eq!(ascii_members(PUNCT), set(PUNCT_SET.chars()));
        assert!(!ascii_members(PUNCT).contains(&'\\'));
        let re = definition_regex(PUNCT).unwrap();
        for c in ["［", "‐", "\u{a0}", "a", "\\", "é"] {
            assert!(!re.is_match(c), "{c}");
        }
        assert_eq!(
            apply(&[PUNCT, " "], r"a[b]c{d}e\f-g_h~i"),
            r"a b c d e\f g h i"
        );
    }

    #[test]
    fn shorthand_dash_is_literal_in_puntotorrent() {
        assert_eq!(ascii_members(r"[\s-_]"), set("\t\n\u{b}\u{c}\r -_".chars()));
        assert_eq!(normalize(r"[\s-_]").unwrap(), r"[\s\x{2D}\x{5F}]");
        assert_eq!(
            apply(&[PUNTO_FULL, "S$1E$3 $2"], "T-05 foo [05x03]"),
            "S05E05  foo "
        );
        assert_eq!(apply(&[PUNTO_FULL, "S$1E$3 $2"], "t_1 x [2/3]"), "S1E2  x ");
        assert_eq!(apply(&[PUNTO_SHORT, " S$1 "], "Show T 07"), "Show  S07 ");
        assert_eq!(apply(&[PUNTO_SHORT, " S$1 "], "Show T.07"), "Show T.07");
    }

    #[test]
    fn literal_braces_follow_is_true_quantifier() {
        let mut c = ctx();
        for (incl, title, description) in [
            (true, "Show {Tags: a}", "Show {Tags: a}"),
            (false, "Show", "Tags: a"),
        ] {
            c.values
                .insert(".Config.incltags".into(), serde_json::json!(incl));
            let title_tpl = "{{ if .Config.incltags }}$1$2{{ else }}$1{{ end }}";
            let desc_tpl = "{{ if .Config.incltags }}{{ else }}$1{{ end }}";
            for (pattern, tpl, want) in [
                (NEKO_TITLE, title_tpl, title),
                (NEKO_TAGS, desc_tpl, description),
            ] {
                let args = [pattern.to_string(), tpl.to_string()];
                let f = Filter::compile("re_replace", &args).unwrap();
                let got = f.apply("Show {Tags: a}", &c).unwrap();
                assert_eq!(
                    got,
                    if incl && pattern == NEKO_TAGS {
                        ""
                    } else {
                        want
                    }
                );
            }
        }
        let re = definition_regex(NEKO_TAGS).unwrap();
        assert_eq!(&re.captures("Show {Tags: a}").unwrap()[1], "Tags: a");
        for q in ["a{2}", "a{2,}", "a{2,3}", "a{2}?", "a{0}", "(ab){1,2}"] {
            assert_eq!(normalize(q).unwrap(), q);
            assert!(matches!(normalize(q).unwrap(), Cow::Borrowed(_)));
        }
        let re = definition_regex("^a{2}$").unwrap();
        assert!(re.is_match("aa") && !re.is_match("a{2}"));
        assert!(definition_regex("^a{2,}$").unwrap().is_match("aaaa"));
        assert!(!definition_regex("^a{2,3}$").unwrap().is_match("aaaa"));
        for literal in [
            "a{", "a{,2}", "a{ 2}", "a{x}", "a{2", "a{2,3", "{", "}{", "a{2,3,}",
        ] {
            let out = normalize(literal).unwrap();
            assert_eq!(out, literal.replace('{', r"\{"), "{literal}");
            let re = definition_regex(&format!("^{literal}$")).unwrap();
            assert!(re.is_match(literal), "{literal}");
            assert!(!re.is_match("a") && !re.is_match("aa"), "{literal}");
        }
        // Escaped and in-class braces, and \p{..}, are not rewritten.
        for same in [r"\{a\}", "[{}]", r"\p{Nd}{2}", "a}"] {
            assert_eq!(normalize(same).unwrap(), same);
        }
        assert!(definition_regex("^a}$").unwrap().is_match("a}"));
    }

    #[test]
    fn classes_follow_scan_char_class() {
        let word = || ('a'..='z').chain('A'..='Z').chain('0'..='9').chain(['_']);
        let cases: Vec<(&str, BTreeSet<char>, bool)> = vec![
            ("[]a]", set([']', 'a']), false),
            ("[a-]", set(['a', '-']), false),
            ("[-a]", set(['-', 'a']), false),
            (r"[\w-]", set(word().chain(['-'])), false),
            ("[a-c-e]", set("abc-e".chars()), false),
            (r"[\x41-\x5A]", set('A'..='Z'), false),
            ("[a^]", set("a^".chars()), false),
            (r"[\--/]", set("-/".chars()), true),
            ("[--/]", set("-./".chars()), true),
            ("[]-a]", set("]^_`a".chars()), true),
            ("[!--]", set('!'..='-'), true),
            (r"[\d-z]", set(('0'..='9').chain("-z".chars())), true),
            (r"[\d--z]", set(('0'..='9').chain('-'..='z')), true),
            ("[a&&b]", set("a&b".chars()), true),
            ("[a~~b]", set("a~b".chars()), true),
            ("[[a]", set("[a".chars()), true),
            ("[[-a]", set('['..='a'), true),
            (r"[\p{Lu}-z]", set(('A'..='Z').chain("-z".chars())), true),
            (r"[\x2D-/]", set("-./".chars()), false),
        ];
        for (pattern, want, rewritten) in cases {
            assert_eq!(ascii_members(pattern), want, "{pattern}");
            let out = normalize(pattern).unwrap();
            assert_eq!(out != pattern, rewritten, "{pattern} -> {out}");
            assert_eq!(
                ascii_members(&format!("(?i){pattern}")),
                fold(&want),
                "(?i){pattern}"
            );
            let negated = pattern.replacen('[', "[^", 1);
            let all = set((0u8..0x80).map(char::from));
            assert_eq!(
                ascii_members(&negated),
                all.difference(&want).copied().collect(),
                "{negated}"
            );
        }
        assert_eq!(ascii_members("[^]a]"), {
            let mut s = set((0u8..0x80).map(char::from));
            s.remove(&']');
            s.remove(&'a');
            s
        });
        assert_eq!(normalize("[]-a]").unwrap(), r"[\x{5D}-a]");
        assert_eq!(normalize(r"[\--/]").unwrap(), r"[\x{2D}\x{2D}\x{2F}]");
        let cyr = definition_regex(r"^[Ѐ-ӿ]$").unwrap();
        for (code, inside) in [(0x3ff, false), (0x400, true), (0x4ff, true), (0x500, false)] {
            assert_eq!(
                cyr.is_match(&char::from_u32(code).unwrap().to_string()),
                inside
            );
        }
        // Non-ASCII literals in a rewritten class stay exact.
        let re = definition_regex("^[«-»&&]$").unwrap();
        assert_eq!(
            normalize("[«-»&&]").unwrap(),
            r"[\x{AB}-\x{BB}\x{26}\x{26}]"
        );
        for c in ["«", "»", "®", "&"] {
            assert!(re.is_match(c), "{c}");
        }
        assert!(!re.is_match("a") && !re.is_match("-") && !re.is_match("¼"));
    }

    #[test]
    fn rejects_unmodelled_or_invalid_syntax() {
        for pattern in [
            r"[a-\d]",
            "[z-a]",
            "[a-[b]]",
            "[a-z-[aeiou]]",
            "[[:alpha:]]",
            "[abc",
            r"[a\",
            "(?x)a",
            "(?x:a)",
            "(?-x)a",
            "(?#c)a",
            "(?n)a",
            "(?U)a",
            "(?P<n>a)",
            r"\x{41}",
            r"\u{41}",
            r"[\x{41}]",
            r"\x4",
            r"\uD800",
            r"[\uDC00]",
            r"\U0001F600",
            r"\pL",
            r"\_",
            r"[\_]",
            r"\<a\>",
            r"\b{start}",
            r"[\b]",
            r"[\e]",
            "😀",
            "[😀]",
            "a{2}+",
            "a**",
            "a+*",
            "a*??",
            "a?{2}",
            "(?<=a)b",
            "(?!a)b",
            r"(a)\1",
            r"\k<n>",
        ] {
            assert!(definition_regex(pattern).is_err(), "{pattern}");
        }
        // ScanBlank skips comments without RegexOptions.IgnorePatternWhitespace.
        assert!(normalize("a(?#b)c").unwrap_err().contains("comment group"));
        for ok in [
            "a*?", "a??", "a{2}?", "(?i)a", "(?i:a)", "(?-i)a", "(?s)a.", "(?m)^a",
        ] {
            assert!(definition_regex(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn bounded_iterative_scanner_and_limits() {
        // Deep brackets are one flat class; no recursion, so a small stack suffices.
        let deep = format!("{}]", "[".repeat(1000));
        let out = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || normalize(&deep).map(Cow::into_owned))
            .unwrap()
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(out, format!("[{}]", r"\x{5B}".repeat(999)));
        assert!(definition_regex(&format!("{}]", "[".repeat(1000)))
            .unwrap()
            .is_match("["));
        // Canonical classes count against the translated cap after the source cap.
        let err = definition_regex(&format!("{}]", "[".repeat(2000))).unwrap_err();
        assert!(err.contains("translation limit"), "{err}");
        assert!(definition_regex(&"{".repeat(4096)).is_ok());
        assert!(definition_regex(&"{".repeat(4097))
            .unwrap_err()
            .contains("source limit"));
        assert!(definition_regex(&format!("{}{}", "(".repeat(100), ")".repeat(100))).is_err());
    }

    #[test]
    fn block_properties_stay_gated_against_rewrites() {
        for pattern in [
            r"\p{IsCyrillic}{",
            r"[\s-_]\p{IsCyrillic}",
            r"[a&&b]\p{IsCyrillic}",
            r"[\p{IsCyrillic}-z]",
            r"[a-b-\p{IsCyrillic}]",
            r"(?i)\p{IsCyrillic}",
        ] {
            assert!(normalize(pattern).is_err(), "{pattern}");
        }
        // Generated nested block classes are emitted, never rescanned as .NET literals.
        assert_eq!(
            normalize(r"[^\P{IsCyrillic}x]{2}").unwrap(),
            r"[^[^\x{0400}-\x{04FF}]x]{2}"
        );
        assert_eq!(
            normalize(r"[]\p{IsCJKUnifiedIdeographs}]").unwrap(),
            r"[][\x{4E00}-\x{9FFF}]]"
        );
        let re = definition_regex(r"^[\p{IsCyrillic}\W]+$").unwrap();
        assert!(re.is_match("А-!") && !re.is_match("Ԁ") && !re.is_match("a"));
    }

    #[test]
    fn corpus_patterns_change_only_where_expected() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/jackett/definitions");
        let mut paths = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "yml"))
            .collect::<Vec<_>>();
        paths.sort();
        assert!(paths.len() > 500);
        let mut uses = BTreeMap::<String, BTreeSet<String>>::new();
        let mut changed_uses = 0;
        for path in &paths {
            let Ok(node) = crate::cardigann::load(path, crate::cardigann::Limits::default()) else {
                continue;
            };
            let file = path.file_stem().unwrap().to_string_lossy().into_owned();
            let mut stack = vec![&node];
            SEEN.with(|s| s.borrow_mut().clear());
            while let Some(n) = stack.pop() {
                match &n.value {
                    crate::cardigann::Value::Mapping(pairs) => {
                        if matches!(n.string("name"), Some("regexp" | "re_replace")) {
                            let args = n.get("args").unwrap();
                            let first = match &args.value {
                                crate::cardigann::Value::Sequence(v) => v[0].text(),
                                _ => args.text(),
                            };
                            let _ = normalize(first.unwrap());
                        }
                        stack.extend(pairs.iter().map(|(_, v)| v));
                    }
                    crate::cardigann::Value::Sequence(v) => stack.extend(v),
                    crate::cardigann::Value::Scalar(s) if s.text.contains("re_replace") => {
                        let _ = Template::compile(&s.text);
                    }
                    _ => {}
                }
            }
            for pattern in SEEN.with(|s| std::mem::take(&mut *s.borrow_mut())) {
                if EXPECTED.iter().any(|(p, ..)| *p == pattern) {
                    changed_uses += 1;
                }
                uses.entry(pattern).or_default().insert(file.clone());
            }
        }
        assert!(uses.len() > 500, "{}", uses.len());
        let mut changed = BTreeMap::new();
        for (pattern, files) in &uses {
            match normalize(pattern) {
                Ok(out) if out == *pattern => {}
                Ok(out) if !pattern.contains(r"\p{Is") && !pattern.contains(r"\P{Is") => {
                    changed.insert(pattern.clone(), (out.into_owned(), files.clone()));
                }
                Ok(_) => {}
                // A new rejection is only acceptable where Rust already rejected the pattern.
                Err(e) => assert!(bounded_regex(pattern).is_err(), "{pattern}: {e}"),
            }
        }
        let want = EXPECTED
            .iter()
            .map(|(p, out, _)| (p.to_string(), out.to_string()))
            .collect::<BTreeMap<_, _>>();
        let got = changed
            .iter()
            .map(|(p, (out, _))| (p.clone(), out.clone()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(got, want);
        let files = changed
            .values()
            .flat_map(|(_, f)| f.iter().cloned())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            files,
            [
                "girotorrent",
                "ilcorsaroblu",
                "jpopsuki",
                "nekobt",
                "puntotorrent"
            ]
            .map(String::from)
            .into()
        );
        assert_eq!(changed_uses, 9);
        for (pattern, out, captures) in EXPECTED {
            assert!(bounded_regex(pattern).is_err(), "{pattern} used to fail");
            let re = definition_regex(pattern).unwrap();
            assert_eq!(re.as_str(), out);
            assert_eq!(re.captures_len(), captures, "{pattern}");
            assert_eq!(
                replace_regex(&re, "none", "$1").unwrap(),
                "none",
                "{pattern}"
            );
        }
    }
}
