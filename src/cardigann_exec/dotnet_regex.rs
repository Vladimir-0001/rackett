//! Definition-sourced .NET regex syntax normalized to Rust `regex` syntax.
//!
//! Source: dotnet/runtime v9.0.0 9d5a6a9, RegexParser.cs. Jackett constructs definition
//! regexes with no options (CardigannIndexer.cs:345/1143/1151). One bounded, iterative
//! scanner reads the original .NET tokens; generated Rust syntax (nested block classes,
//! canonical classes) is only ever appended to the output and never rescanned. Constructs
//! whose .NET and Rust readings diverge are rewritten only when the .NET reading is exact
//! from source, and otherwise rejected here. Look-around, atomic groups and numbered
//! backreferences are classified for the fancy-regex fallback (see [`scan`] and
//! docs/dotnet-regex-lookaround.md); balancing groups and conditionals are rejected here.
//! Other constructs Rust does not support (octal, `\G`, ...) are left for Rust to reject;
//! constructs the scanner rejects first (e.g. `(?#..)`, `\'name'` references) get this
//! module's message. Every non-BMP pattern character is rejected: a deliberately conservative
//! scope limit,
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
        // Also literal there: White_Space is never a .NET boundary word character (L, Mn,
        // Nd, Pc, ZWJ/ZWNJ). Rust rejects the escape, so it is re-emitted as `\x{..}`.
        c if c.is_whitespace() => Escape::Char(c),
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
        if escaped && !chars[start + 1].is_ascii() {
            c.hazard = Some("escaped non-ASCII literal");
        }
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
/// property, a literal `{` (not IsTrueQuantifier), an escaped non-ASCII whitespace literal,
/// or a character class Rust would read differently. Block properties stay rejected next to
/// any other rewrite, inline options, comments, look-around or backreferences, as before.
#[cfg(test)]
pub(super) fn normalize(pattern: &str) -> Result<Cow<'_, str>, String> {
    scan(pattern).map(|n| n.text)
}

/// A normalized definition regex and the engine class it needs.
pub(super) struct Normalized<'a> {
    pub(super) text: Cow<'a, str>,
    /// Look-around, atomic groups or backreferences, which only a backtracking engine runs
    /// (docs/dotnet-regex-lookaround.md). Everything else stays on Rust's linear engine.
    pub(super) backtracking: bool,
}

/// [`normalize`], also classifying the constructs that need a backtracking engine. Those are
/// accepted only together with syntax both engines read as .NET does: other escapes that
/// Rust rejects but fancy-regex reads with its own (Oniguruma/PCRE) meaning stay rejected.
pub(super) fn scan(pattern: &str) -> Result<Normalized<'_>, String> {
    #[cfg(test)]
    tests::SEEN.with(|seen| seen.borrow_mut().push(pattern.to_owned()));
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(pattern.len());
    let (mut rewritten, mut has_block, mut gate) = (false, false, None::<&str>);
    // First construct needing backtracking; first escape only fancy-regex would accept.
    let (mut backtracking, mut foreign) = (None::<&str>, None::<String>);
    // Capture groups as .NET counts them, numbered backreferences, and any `i` option.
    let (mut unnamed, mut named, mut casei) = (0usize, 0usize, false);
    let mut backrefs = Vec::new();
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
                let (escape, mut end) = escape(&chars, i, false)?;
                let kind = chars.get(i + 1).copied().unwrap_or('\\');
                match escape {
                    Escape::Block(range, negated) => {
                        push_block(&mut out, range, negated);
                        has_block = true;
                    }
                    Escape::Char(c) if !kind.is_ascii() => {
                        push_class_char(&mut out, c);
                        rewritten = true;
                    }
                    // ScanBasicBackslash :1137: `\1`-`\9` start a decimal group reference.
                    Escape::Verbatim if matches!(kind, '1'..='9') => {
                        let digits = chars[i + 1..]
                            .iter()
                            .take_while(|c| c.is_ascii_digit())
                            .collect::<String>();
                        // Overflowing numbers cannot name a group (checked below).
                        backrefs.push(digits.parse::<usize>().unwrap_or(usize::MAX));
                        backtracking = backtracking.or(Some("backreference"));
                        gate = Some("backreference");
                        end = i + 1 + digits.len();
                        out.extend(&chars[i..end]);
                    }
                    // Assertions both engines read as .NET does (RegexNode Boundary,
                    // NonBoundary, Beginning, End). `\G`, `\Z`, `\k`, octal, `\e`, ... differ
                    // in or are absent from Rust, and fancy-regex has its own meanings.
                    Escape::Verbatim if !matches!(kind, 'b' | 'B' | 'A' | 'z') => {
                        foreign.get_or_insert_with(|| format!("escape \\{kind}"));
                        out.extend(&chars[i..end]);
                    }
                    _ => out.extend(&chars[i..end]),
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
                casei |= flags.iter().take_while(|c| ***c != '-').any(|c| **c == 'i');
                let plain = match rest {
                    [':', ..] => true,
                    ['<', n, ..] => n.is_alphanumeric() || *n == '_',
                    _ => false,
                };
                // ScanGroupOpen :766-1010. Balancing groups and conditionals have no Rust
                // equivalent; `(?'name')` and digit names have different or no Rust readings.
                match rest {
                    ['<', '=' | '!', ..] => backtracking = backtracking.or(Some("look-behind")),
                    ['=' | '!', ..] => backtracking = backtracking.or(Some("look-ahead")),
                    ['>', ..] => backtracking = backtracking.or(Some("atomic group")),
                    ['(', ..] => return err("conditional group (?(...)"),
                    ['\'', ..] => return err("(?'name') group"),
                    ['<', '-', ..] => return err("balancing group (?<-name>)"),
                    ['<', n, ..] if n.is_ascii_digit() => return err("numbered group (?<N>)"),
                    ['<', ..] if plain => {
                        let name = rest[1..].iter().take_while(|c| **c != '>');
                        if name.clone().any(|c| *c == '-') {
                            return err("balancing group (?<name1-name2>)");
                        }
                        named += 1;
                    }
                    _ => {}
                }
                if !plain {
                    gate = Some("inline option, comment, or look-around group");
                }
                out.push_str("(?");
                i += 2;
            }
            '(' => {
                unnamed += 1;
                out.push(c);
                i += 1;
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
    if let Some(what) = backtracking {
        if let Some(escape) = foreign {
            return err(&format!("{escape} with a {what}"));
        }
        // RegexParser numbers unnamed groups before named ones; Rust numbers both in order.
        if !backrefs.is_empty() && named > 0 {
            return err("numbered backreference with named groups");
        }
        // IsCaptureSlot, else ScanBasicBackslash throws (<= 9) or reads octal (> 9).
        if backrefs.iter().any(|n| *n > unnamed) {
            return err("backreference to an undefined group");
        }
        // .NET compares case-insensitive references with its culture case table.
        if !backrefs.is_empty() && casei {
            return err("case-insensitive backreference");
        }
    }
    let text = if rewritten || has_block {
        Cow::Owned(out)
    } else {
        debug_assert_eq!(out, pattern);
        Cow::Borrowed(pattern)
    };
    Ok(Normalized {
        text,
        backtracking: backtracking.is_some(),
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
    /// Pinned ebookbay/btarg: escaped no-break spaces, which .NET reads as literals.
    const EBOOK: &str = "E book under:\\s+(.+?)\\s?\\\u{a0}";
    const BTARG_NBSP: &str = "\\\u{a0}(\\d+)\\\u{a0}";
    /// Every corpus rewrite: (pattern, normalized, captures_len).
    const EXPECTED: [(&str, &str, usize); 7] = [
        (PUNCT, PUNCT_OUT, 1),
        (
            PUNTO_FULL,
            r"(?i)T[\s\x{2D}\x{5F}]?(\d{1,2})(.*)\[(\d{1,2})[x|\/|\\](\d{1,2})\]",
            5,
        ),
        (PUNTO_SHORT, r"(?i)T[\s\x{2D}\x{5F}]?(\d{1,2})\b", 2),
        (NEKO_TITLE, r"(.+?)( \{Tags:.+)", 3),
        (NEKO_TAGS, r".+ \{(Tags:.+?)}", 2),
        (EBOOK, r"E book under:\s+(.+?)\s?\x{A0}", 2),
        (BTARG_NBSP, r"\x{A0}(\d+)\x{A0}", 2),
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
        assert_eq!(
            re.captures("Show {Tags: a}").unwrap().unwrap().get(1),
            Some("Tags: a")
        );
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
            r"\k<n>",
        ] {
            assert!(definition_regex(pattern).is_err(), "{pattern}");
        }
        // ScanBlank skips comments without RegexOptions.IgnorePatternWhitespace.
        assert!(normalize("a(?#b)c").unwrap_err().contains("comment group"));
        for ok in [
            "a*?", "a??", "a{2}?", "(?i)a", "(?i:a)", "(?-i)a", "(?s)a.", "(?m)^a", "(?<=a)b",
            "(?!a)b", r"(a)\1",
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
                "btarg",
                "ebookbay",
                "girotorrent",
                "ilcorsaroblu",
                "jpopsuki",
                "nekobt",
                "puntotorrent"
            ]
            .map(String::from)
            .into()
        );
        assert_eq!(changed_uses, 11);
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

    /// The pinned French/Polish tracker filters, applied as `re_replace` with the
    /// `multilanguage` setting. Expected values follow .NET semantics: `.` excludes only
    /// `\n`, `\b` and `(?i)` apply inside the look-ahead, and look-ahead text is not consumed.
    const MULTI_FR: &str = r"(?i)\b(MULTI(?!.*(?:FRENCH|ENGLISH|VOSTFR)))\b";
    const MULTI_FR_EXT: &str =
        r"(?i)\b(MULTI(?!.*(?:FRENCH|ENGLISH|VOSTFR|VF2|VFF|VFQ|VOQ|VFI|VOF)))\b";
    const MULTI_PL: &str = r"(?i)\b(MULTI(?!.*(?:POLISH|ENGLISH|\bPL\b)))\b";

    #[test]
    fn multi_negative_lookahead_follows_dotnet() {
        let lang = "MULTI.FRENCH";
        for (pattern, input, want) in [
            (
                MULTI_FR,
                "Movie.2023.MULTI.1080p.WEB",
                "Movie.2023.MULTI.FRENCH.1080p.WEB",
            ),
            (
                MULTI_FR,
                "Movie.2023.MULTI.FRENCH.1080p",
                "Movie.2023.MULTI.FRENCH.1080p",
            ),
            (
                MULTI_FR,
                "Movie.2023.multi.1080p.English",
                "Movie.2023.multi.1080p.English",
            ),
            (MULTI_FR, "Film MULTi VOSTFR 720p", "Film MULTi VOSTFR 720p"),
            (
                MULTI_FR,
                "Show.S01.multi.720p",
                "Show.S01.MULTI.FRENCH.720p",
            ),
            // `\b` needs a non-word neighbour; "MULTiSUBS" has none after MULTI.
            (MULTI_FR, "Movie.MULTiSUBS.1080p", "Movie.MULTiSUBS.1080p"),
            // Substring search: FRENCHY still contains FRENCH.
            (
                MULTI_FR,
                "Movie MULTI 1080p FRENCHY",
                "Movie MULTI 1080p FRENCHY",
            ),
            // `.` stops at `\n`, so the look-ahead does not see the next line.
            (
                MULTI_FR,
                "Movie MULTI\nFRENCH",
                "Movie MULTI.FRENCH\nFRENCH",
            ),
            (MULTI_FR, "MULTI.FRENCH.MULTI", "MULTI.FRENCH.MULTI.FRENCH"),
            (MULTI_FR, "MULTI.x.MULTI.vostfr", "MULTI.x.MULTI.vostfr"),
            // VFF is only in the extended list.
            (
                MULTI_FR,
                "Movie.MULTI.VFF.1080p",
                "Movie.MULTI.FRENCH.VFF.1080p",
            ),
            (
                MULTI_FR_EXT,
                "Movie.MULTI.VFF.1080p",
                "Movie.MULTI.VFF.1080p",
            ),
            (
                MULTI_FR_EXT,
                "Movie.MULTI.VF2.TRUEFRENCH",
                "Movie.MULTI.VF2.TRUEFRENCH",
            ),
            (MULTI_FR_EXT, "Movie.MULTI.WEB", "Movie.MULTI.FRENCH.WEB"),
            // Non-ASCII letters are word characters for both \b readings.
            (MULTI_FR, "ÉtéMULTI 1080p", "ÉtéMULTI 1080p"),
            (MULTI_FR, "Été MULTIé 1080p", "Été MULTIé 1080p"),
            (MULTI_FR, "Été MULTI 1080p", "Été MULTI.FRENCH 1080p"),
            (
                MULTI_FR,
                "Été.MULTI.Française",
                "Été.MULTI.FRENCH.Française",
            ),
            // `\bPL\b` inside the look-ahead.
            (MULTI_PL, "Film.MULTI.PL.1080p", "Film.MULTI.PL.1080p"),
            (
                MULTI_PL,
                "Film.MULTI.PLAY.1080p",
                "Film.MULTI.FRENCH.PLAY.1080p",
            ),
            (MULTI_PL, "Film.MULTI.polish", "Film.MULTI.polish"),
            (MULTI_PL, "Film.MULTI.pl", "Film.MULTI.pl"),
            (MULTI_PL, "Film.MULTI.Żółw", "Film.MULTI.FRENCH.Żółw"),
        ] {
            assert_eq!(apply(&[pattern, lang], input), want, "{pattern} {input:?}");
        }
        let re = definition_regex(MULTI_FR).unwrap();
        assert!(re.is_backtracking());
        assert_eq!(re.captures_len(), 2);
        assert_eq!(re.as_str(), MULTI_FR);
    }

    #[test]
    fn other_pinned_lookaround_and_backreference_patterns() {
        // (pattern, replacement, input, .NET Regex.Replace result)
        for (pattern, rep, input, want) in [
            // amigosshare: positive look-ahead with `$`.
            (
                r#"(\{"id":\d+,.*?\})(?=,\{"id":\d+,|$)"#,
                "<$1>",
                r#"{"id":1,"a":"x"},{"id":2,"b":"y"}"#,
                r#"<{"id":1,"a":"x"}>,<{"id":2,"b":"y"}>"#,
            ),
            // animetosho-xyz: constant-width look-behind and look-ahead.
            (
                r"(?i)(?<![A-Za-z0-9])(Movies?|OVA|OAV)(?![A-Za-z0-9])",
                "[$1]",
                "Movie OVAs ova MOVIES oav2 xMovie",
                "[Movie] OVAs [ova] [MOVIES] oav2 xMovie",
            ),
            // arabp2p
            (
                r"(?i)\bS(\d)(?=E)",
                "S0$1",
                "Show s1E02 S2x03 S3e04",
                "Show S01E02 S2x03 S03e04",
            ),
            // arabscene: look-behind and look-ahead around consumed whitespace.
            (
                r"(?<=\d)\s+(?=\d)",
                "",
                "Size 1 024 MB 2 a 3  4",
                "Size 1024 MB 2 a 34",
            ),
            // btarg
            (
                r"(?i)(S(\d{1,3}))(?![\dE])",
                "$1E01",
                "Show S02 s03E04 S1234",
                "Show S02E01 s03E04 S1234",
            ),
            // hdonly: numbered backreference.
            (
                r"(\.MULTI)\1",
                ".MULTI",
                "Movie.MULTI.MULTI.1080p",
                "Movie.MULTI.1080p",
            ),
            (
                r"(\.MULTI)\1",
                ".MULTI",
                "Movie.MULTI.multi",
                "Movie.MULTI.multi",
            ),
            // nyaasi: lazy gap then a negative look-ahead.
            (
                r"(?i)\b(?:S|Seasons?)\s?(\d+(?:-\d+)?)(?:.+?)(?:EP|Episodes?)\s?(\d+(?:-\d+)?)\b(?!(?:-\d+)?\sS\d+(?:-\d+)?E\d+(?:-\d+)?)",
                "S$1E$2",
                "Season 2 - Episode 5 [1080p]",
                "S2E5 [1080p]",
            ),
            (
                r"(?i)\b(?:S\s|Seasons?\s?)(\d+(?:-\d+)?)\b(?!(?:-\d+)?\s(?:EP|Episodes?)?\s?(?:\d+(?:-\d+)?)?\s?S\d+(?:E\d+(?:-\d+)?)?)",
                "S$1",
                "Season 3 complete / Season 4 S04E01",
                "S3 complete / Season 4 S04E01",
            ),
        ] {
            assert_eq!(apply(&[pattern, rep], input), want, "{pattern}");
        }
        // hdolimpo: group 1 sits inside a negative look-behind, so it never participates
        // in a successful match; replacements use $2 (a .NET numbering match: no named groups).
        let hdolimpo = r"(?<!(SPANiSH|ENGLiSH|CATALAN|FRENCHx|JAPANES|iTALiAN|RUSSiAN|GERMANx)[\s.])\b(ENGLiSH|CATALAN|FRENCHx|JAPANES|iTALiAN|RUSSiAN|GERMANx)\b(?![\s.]SPANiSH)";
        for (input, want) in [
            ("Movie ENGLiSH 1080p", "Movie MULTi SPANiSH ENGLiSH 1080p"),
            ("Movie SPANiSH ENGLiSH 1080p", "Movie SPANiSH ENGLiSH 1080p"),
            ("Movie ENGLiSH.SPANiSH", "Movie ENGLiSH.SPANiSH"),
            ("Movie.CATALAN.x", "Movie.MULTi SPANiSH CATALAN.x"),
            ("Movie english", "Movie english"),
        ] {
            assert_eq!(
                apply(&[hdolimpo, "MULTi SPANiSH $2"], input),
                want,
                "{input}"
            );
            assert_eq!(
                apply(&[hdolimpo, "[$1]"], input).contains("[]"),
                want != input
            );
        }
        // ebookbay: escaped no-break space is a literal on the linear engine.
        let ebook = Filter::compile("regexp", &[EBOOK.into()]).unwrap();
        assert_eq!(
            ebook
                .apply("E book under:  Fiction\u{a0}more", &ctx())
                .unwrap(),
            "Fiction"
        );
        assert!(!definition_regex(EBOOK).unwrap().is_backtracking());
        let btarg = Filter::compile("regexp", &[BTARG_NBSP.into()]).unwrap();
        assert_eq!(btarg.apply("Seeds\u{a0}42\u{a0}", &ctx()).unwrap(), "42");
        assert_eq!(btarg.apply("Seeds 42 ", &ctx()).unwrap(), "");
        assert_eq!(normalize("[\\\u{3000}]").unwrap(), r"[\x{3000}]");
        assert_eq!(normalize("\\\u{2028}").unwrap(), r"\x{2028}");
        // Non-whitespace non-ASCII escapes stay rejected (\é is a .NET word-character error).
        assert!(definition_regex("\\é").is_err());
    }

    #[test]
    fn atomic_groups_and_lookaround_captures() {
        let find = |p: &str, s: &str| {
            definition_regex(p).unwrap().captures(s).unwrap().map(|c| {
                (0..3)
                    .map(|i| c.get(i).map(String::from))
                    .collect::<Vec<_>>()
            })
        };
        // (?>...) never gives back: .NET finds no match where a plain group would.
        assert_eq!(find("(?>a+)ab", "aaab"), None);
        assert!(find("(?:a+)ab", "aaab").is_some());
        assert_eq!(find("a(?>bc|b)c", "abc"), None);
        assert_eq!(
            find("(?>a+)b", "xaab"),
            Some(vec![Some("aab".into()), None, None])
        );
        // Captures inside a successful positive look-ahead are kept; inside a negative
        // look-around they never are.
        assert_eq!(
            find(r"(?=(\d+))\d", "a42"),
            Some(vec![Some("4".into()), Some("42".into()), None])
        );
        assert_eq!(
            find(r"(?<=(x))y(?!(z))", "xy"),
            Some(vec![Some("y".into()), Some("x".into()), None])
        );
        // Backreference to a group that did not participate fails, as in .NET.
        assert_eq!(find(r"(?:(a)|b)\1", "b"), None);
        assert_eq!(
            find(r"(a)?b\1?", "b"),
            Some(vec![Some("b".into()), None, None])
        );
    }

    #[test]
    fn backtracking_engine_keeps_dotnet_word_and_case_semantics() {
        // Same Unicode \b, \w, \d and simple case folding as the linear engine (the module's
        // documented .NET compatibility level), including inside the backtracking VM.
        for (pattern, input, matched) in [
            (r"(?i)\bmulti\b(?!x)", "ÉTÉ MULTI x", true),
            (r"(?i)\bmulti\b(?!x)", "éMULTI", false),
            (r"(?i)\bmulti\b(?!x)", "MULTIé", false),
            (r"(?i)\bmulti\b(?!x)", "MULTI١", false),
            (r"(?i)(?<=é)t", "ÉT", true),
            (r"(?i)été(?=!)", "ÉTÉ!", true),
            (r"(?i)k(?=!)", "\u{212a}!", true),
            (r"(?-i)k(?=!)", "K!", false),
            (r"\w+(?=!)", "żółw!", true),
            (r"\d(?=!)", "٣!", true),
            (r"(?<=\s)x", "\u{a0}x", true),
            (r"^(?=a).$", "a\n", false),
        ] {
            let re = definition_regex(pattern).unwrap();
            assert!(re.is_backtracking());
            assert_eq!(re.is_match(input), matched, "{pattern} {input:?}");
        }
    }

    /// Every pinned pattern the linear engine accepts, wrapped so fancy-regex must run it in
    /// its VM, gives the same matches and replacements: both engines read normalized syntax
    /// identically.
    #[test]
    fn backtracking_engine_agrees_with_linear_engine_on_corpus_patterns() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/jackett/definitions");
        SEEN.with(|s| s.borrow_mut().clear());
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "yml") {
                if let Ok(node) = crate::cardigann::load(&path, crate::cardigann::Limits::default())
                {
                    let _ = crate::cardigann_exec::Plan::compile(&node);
                }
            }
        }
        let patterns = SEEN
            .with(|s| std::mem::take(&mut *s.borrow_mut()))
            .into_iter()
            .collect::<BTreeSet<_>>();
        let inputs = [
            "",
            "Movie.Title.2023.MULTI.1080p.WEB-DL.x264-GRP",
            "Série Été S01E02 VOSTFR 720p [Rus, Eng] (2019)",
            "Фильм / Film (2019) BDRip 1080p | Сезон 1, Серии 1-10 из 12",
            r#"<a href="details.php?id=123&amp;x=1">Name</a> 1.5 GB 2024-01-02 10:00 +0100"#,
            "S01-S03 Episode 5 - 12 [1080p] {Tags: a, b}",
            "ſKK ß straße İstanbul ǅ ﬁ",
            "line1\nline2\r\n 12,345 seeds; 3 leechers",
            "日本語のタイトル 第1話 (BD 1920x1080) 【中文】",
            "Uploaded 3 days ago by user_1 - 700.5 MiB, ratio 1.0",
        ];
        let mut compared = 0;
        for pattern in &patterns {
            let Ok(linear) = definition_regex(pattern) else {
                continue;
            };
            if linear.is_backtracking() {
                continue;
            }
            let wrapped = format!(r"(?:{pattern})(?!￿)");
            let Ok(vm) = definition_regex(&wrapped) else {
                continue;
            };
            assert!(vm.is_backtracking(), "{wrapped}");
            assert_eq!(vm.captures_len(), linear.captures_len(), "{pattern}");
            for input in inputs {
                let spans = |re: &super::super::filters::DefinitionRegex| {
                    re.captures(input).unwrap().map(|c| {
                        (0..re.captures_len())
                            .map(|i| c.get(i).map(String::from))
                            .collect::<Vec<_>>()
                    })
                };
                assert_eq!(spans(&vm), spans(&linear), "{pattern} on {input:?}");
                assert_eq!(
                    replace_regex(&vm, input, "<$0|$1>"),
                    replace_regex(&linear, input, "<$0|$1>"),
                    "{pattern} on {input:?}"
                );
            }
            compared += 1;
        }
        assert!(compared > 450, "{compared}");
    }

    #[test]
    fn backtracking_rejects_what_it_cannot_run_as_dotnet() {
        for (pattern, why) in [
            // hawke-uno: .NET balancing groups and a group conditional.
            (
                r"\(((?>[^()]+|(?<o>)\(|(?<-o>)\))*(?(o)(?!)))(?<!\bHONE)(?<=\s*- [\w]+)\)$",
                "balancing group",
            ),
            ("(?<a-b>x)", "balancing group"),
            ("(?(a)b|c)", "conditional group"),
            ("(?'n'a)", "(?'name') group"),
            ("(?<1>a)", "numbered group"),
            // fancy-regex reads these with Oniguruma/PCRE meanings.
            (r"\Gx(?=a)", r"escape \G with a look-ahead"),
            (r"x\Z(?<=x)", r"escape \Z with a look-behind"),
            (r"(?<n>a)\k<n>(?=a)", r"escape \k with a look-ahead"),
            (r"\e(?=a)", r"escape \e with a look-ahead"),
            (r"\0(?=a)", r"escape \0 with a look-ahead"),
            (r"\K(?=a)", r"escape \K with a look-ahead"),
            (r"\h(?=a)", r"escape \h with a look-ahead"),
            // Numbering, undefined groups and culture-dependent case comparison.
            (r"(?<n>a)(b)\1", "numbered backreference with named groups"),
            (r"(a)\2", "backreference to an undefined group"),
            (r"(a)\10", "backreference to an undefined group"),
            (r"(?i)(a)\1", "case-insensitive backreference"),
            (r"(?i:x)(a)\1", "case-insensitive backreference"),
            // Block properties stay gated next to any backtracking construct.
            (r"\p{IsCyrillic}(?=a)", "with a .NET block property"),
            (r"(\p{IsCyrillic})\1", "with a .NET block property"),
        ] {
            let e = definition_regex(pattern).unwrap_err();
            assert!(e.contains(why), "{pattern}: {e}");
        }
        // Variable-width look-behind (valid .NET) is rejected by the engine configuration.
        for pattern in [
            r"(?<=a+)b",
            r"(?<=\s*- [\w]+)\)",
            r"(?<=a?)b",
            r"(?<!(ab)*)c",
        ] {
            let e = definition_regex(pattern).unwrap_err();
            assert!(
                e.starts_with("unsupported/invalid .NET regex"),
                "{pattern}: {e}"
            );
        }
        // Alternatives of different constant widths are each checked, as .NET's right-to-left
        // look-behind does.
        let re = definition_regex(r"(?<!ab|c)d").unwrap();
        for (input, matched) in [("abd", false), ("cd", false), ("bd", true), ("xd", true)] {
            assert_eq!(re.is_match(input), matched, "{input}");
        }
        // Still allowed: case-sensitive scope after (?-i), and assertions both engines share.
        assert!(definition_regex(r"(?-i)(a)\1").is_ok());
        assert!(definition_regex(r"\Ax\b\B(?=y)\z").is_ok());
        assert!(definition_regex(r"(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\10").is_ok());
    }

    #[test]
    fn backtrack_limit_is_a_bounded_error_not_a_hang() {
        // Two ways to match each `a` and a final failure: 2^40 paths without the limit.
        let pattern = r"^(?:a|(?=a)a)*b";
        let input = "a".repeat(40);
        let started = std::time::Instant::now();
        let re = definition_regex(pattern).unwrap();
        assert!(re.is_backtracking());
        let limit = "regex backtrack limit exceeded";
        assert_eq!(re.captures(&input).err().as_deref(), Some(limit));
        assert_eq!(replace_regex(&re, &input, "x").unwrap_err(), limit);
        let regexp = Filter::compile("regexp", &[format!("({pattern})")]).unwrap();
        assert_eq!(regexp.apply(&input, &ctx()).unwrap_err(), limit);
        let re_replace = Filter::compile("re_replace", &[pattern.into(), "x".into()]).unwrap();
        assert_eq!(re_replace.apply(&input, &ctx()).unwrap_err(), limit);
        let template = format!(r#"{{{{ re_replace .Query.Q "{pattern}" "x" }}}}"#);
        let mut c = ctx();
        c.values.insert(".Query.Q".into(), serde_json::json!(input));
        assert_eq!(
            Template::compile(&template)
                .unwrap()
                .render(&c)
                .unwrap_err(),
            limit
        );
        // The same pattern succeeds when the search is short.
        assert_eq!(apply(&[pattern, "x"], "aaab"), "x");
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        // Backtracking regexes have a tighter input cap than the linear engine.
        let big = "a".repeat(super::super::filters::MAX_BACKTRACKING_INPUT + 1);
        let e = definition_regex("a(?=a)").unwrap().captures(&big).err();
        assert_eq!(
            e.as_deref(),
            Some("backtracking regex input limit exceeded")
        );
        assert!(definition_regex("aa").unwrap().captures(&big).is_ok());
        const { assert!(super::super::filters::BACKTRACK_LIMIT == 100_000) };
    }
}
