# .NET look-around, atomic groups and backreferences in definition regexes

## Purpose

Jackett runs definition regexes (`regexp`/`re_replace` filters, keywordsfilters,
templated `re_replace`) on .NET's backtracking `System.Text.RegularExpressions`
with no options (`CardigannIndexer.cs:345/1143/1151` at the pinned Jackett).
Rackett's linear-time `regex` crate rejects look-around, atomic groups and
backreferences. At commit `0deda9d` that rejected 37 pinned definitions at
`Plan::compile`. Twenty-seven of them share the French/Polish tracker filter
`(?i)\b(MULTI(?!.*(?:FRENCH|ENGLISH|...)))\b`.

This change runs those constructs with .NET semantics. After it, 36 of the 37
definitions no longer fail on a regex. Only `hawke-uno` stays rejected (see below).

## Design

One normalizer, two engines. `dotnet_regex::scan` reads the original .NET
tokens once, as before. It returns the normalized Rust syntax and a
`backtracking` flag. `filters::definition_regex` then compiles:

| Pattern needs | Engine | Compiled as |
| --- | --- | --- |
| Nothing beyond the `regex` crate | `regex` (linear time) | `DefinitionRegex::Linear` |
| Look-ahead `(?=` `(?!`, look-behind `(?<=` `(?<!`, atomic `(?>`, numbered backreference `\N` | `fancy-regex` 0.20 (backtracking) | `DefinitionRegex::Backtracking` |

Both engines receive the **same normalized text**, so every existing .NET→Rust
rewrite still applies: literal `{`, canonical classes, block properties, and
escaped no-break spaces (below). Patterns that `regex` accepts never switch
engine. Their behaviour is unchanged.

`DefinitionRegex` is the only compiled type for definition regexes. It is used
by `Filter` (`regexp`, `re_replace`, so also keywordsfilters and every field
filter compiled through `Filter::compile_for`) and by the template
`re_replace` expression. Every match API returns a `Result`, because only the
backtracking engine can fail at match time. Internal, fixed Rust-syntax regexes
(`bounded_regex`, row filters, dates, `diacritics`) stay on `regex::Regex`.

fancy-regex parses the whole pattern itself. It runs look-around, atomic groups
and backreferences in a backtracking VM and hands the plain sub-expressions to
the `regex` crate. So `\b`, `\w`, `\d`, `\s`, `\p{..}` and `(?i)` keep exactly the
Unicode semantics `regex` already gives. Those are the compatibility level
documented in cardigann.md: the same Unicode-version, case-folding
and final-newline limits as before. Tests check this inside the VM with non-ASCII
letters (`é`, `ż`, Arabic-Indic digits, the Kelvin sign under `(?i)`) and `\b`
next to non-ASCII word characters. A differential test wraps every pinned
pattern the linear engine accepts (more than 450) as `(?:P)(?!￿)`, which
forces the VM path. On a set of sample inputs it then checks that the VM returns
the same captures and `re_replace` output as the linear engine.

## .NET equivalence decisions

Supported, with source reasoning (`RegexParser.cs`, dotnet/runtime v9.0.0):

- **Look-ahead and constant-width look-behind.** For an assertion, only whether it
  matches matters. A fixed-width look-behind matches the same substrings in
  .NET's right-to-left scan and fancy-regex's step-back check. Alternatives of
  different constant widths (`(?<!ab|c)`) are each checked. Captures inside a
  successful positive look-around are kept. Captures inside a negative
  look-around never take part in a match (hdolimpo's `$1`).
- **Atomic groups** `(?>...)` never backtrack into the group in either engine.
- **Numbered backreferences** `\1`..`\N`, case-sensitive only. A reference to a
  group that did not take part fails to match, as in .NET.
- **Escaped non-ASCII whitespace** (`\` + U+00A0 and the other White_Space
  characters). .NET `ScanCharEscape` reads an escaped non-word character as a
  literal, and White_Space characters are never .NET boundary word characters.
  Rust rejects the escape, so the normalizer re-emits it as `\x{..}` on either
  engine. This fixes ebookbay and btarg's second filter. Other escaped
  non-ASCII characters are still rejected.

Rejected with an explicit `unsupported/invalid .NET regex: ...` message:

| Construct | Why |
| --- | --- |
| Balancing groups `(?<-o>)`, `(?<a-b>)` | No Rust engine models .NET capture stacks. |
| Conditionals `(?(o)...)` | fancy-regex has conditionals, but with Oniguruma/PCRE name-vs-expression rules, not .NET's. |
| `(?'name')` groups, digit-named groups `(?<1>)` | Different or no Rust reading. |
| Variable-width look-behind (`(?<=a+)`, `(?<=a?)`) | Valid .NET, but fancy-regex's variable-look-behind feature stops collecting candidates when the backtrack budget runs out, instead of failing. That would give a wrong answer, not an error. The feature is disabled in `Cargo.toml` (`default-features = false`). |
| Numbered backreference when the pattern has named groups | .NET numbers unnamed groups first, then named groups. fancy-regex and Rust number them in order. |
| Backreference to an undefined group, including multi-digit (`\10` with fewer groups) | .NET throws (≤ 9) or reads octal (> 9). |
| Backreference in a pattern with any `i` option | .NET compares with culture case tables. Not claimed. |
| `\G`, `\Z`, `\k<..>`, `\0`, `\e`, `\K`, `\h`, ... next to a backtracking construct | fancy-regex would accept these with its own meanings. Only `\b`, `\B`, `\A`, `\z` and validated `\N` are allowed. Without a backtracking construct, `regex` keeps rejecting them as before. |
| .NET block properties (`\p{IsCyrillic}`) next to a backtracking construct | Same gate as other rewrites next to block properties. |

`hawke-uno` stays rejected. Its title filter
`\(((?>[^()]+|(?<o>)\(|(?<-o>)\))*(?(o)(?!)))(?<!\bHONE)(?<=\s*- [\w]+)\)$`
matches arbitrarily nested parentheses with a balancing group and a
conditional, and also uses a variable-width look-behind. No Rust regex engine
can express arbitrary-depth balancing. An exact rewrite would need a hand-written
matcher, so the definition keeps a clear `balancing group (?<-name>)` diagnostic.

## Bounds

- **Backtrack limit:** `filters::BACKTRACK_LIMIT = 100_000` steps per search. A
  search is one `captures` call covering all start positions. Running out of
  budget returns `regex backtrack limit exceeded`, a bounded extraction error.
  It never hangs and never returns a truncated result. `re_replace` runs at most
  8192 searches (existing match cap), and each has its own budget.
- **Input cap:** backtracking regexes refuse inputs over
  `filters::MAX_BACKTRACKING_INPUT = 64 KiB` with
  `backtracking regex input limit exceeded`. Look-around sub-expressions
  rescan text from each attempted position. Even under the backtrack limit, a
  search can therefore be quadratic in input length, so the cap bounds that
  work. Linear regexes keep the existing 1 MiB filter input limit.
- **Unchanged:** 4096-byte source limit, 8192-byte translated limit, 1 MiB
  compiled-size and DFA limits (also used for fancy-regex's delegates), group
  nesting ≤ 64 (fancy-regex's parser recursion limit), output and match caps.

A test runs a 2^40-path pattern (`^(?:a|(?=a)a)*b` on 40 `a`s). It fails with
the limit error through every entry point: regexp, re_replace and template.

## Validation

Run with `CARGO_BUILD_JOBS=3`:

- `cargo test --locked --lib`: new tests in `src/cardigann_exec/dotnet_regex.rs`:
  - MULTI on 23 French/Polish release names: case, `\b` with non-ASCII, `.` vs `\n`, substring look-ahead, both language lists.
  - Every other pinned look-around/backreference pattern.
  - Atomic groups and look-around captures.
  - Unicode `\b`/`\w`/`(?i)` inside the VM.
  - The corpus differential test.
  - Rejections.
  - The backtrack and input limits.
- `cargo test --locked --test cardigann_regex_lookaround`:
  - Prints the status of each of the 37 definitions.
  - Asserts that only `hawke-uno` still fails on a regex, and that the 9 others not yet compiling stop at `response.type: unsupported xml`, a separate workstream.
  - Extracts synthetic torrent9 rows with `multilang` on and off.
- `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings`.

The expected values come from the .NET rules cited above. No .NET runtime was
available on the build machine to cross-check them.

Status of the 37 after this change:

- **Compile fully (27):** abnormal-api, amigosshare, arabp2p, arabscene, btarg,
  cinemamovies, devil-torrents, ebookbay, electro-torrent, ex-torrenty,
  g3minitr4ck3r-api, generationfree-api, hdolimpo-api, hdonly-api, helltorrents,
  lesaloon, nyaasi, polishtorrent, polishtracker-api, teamctgame, teamflix,
  theoldschool-api, torrent9, v3x, wihd, world-torrent, xtorrenty.
- **Regexes compile, XML responses next (9):** animetosho-xyz, astratorrent, c411,
  crazyspirits-api, draupnirr, memphis, nostradamus, tr4ker, yggreborn-api.
- **Still regex-rejected (1):** hawke-uno.

## Remaining work

- Fixture agents admit the newly compiling definitions (`src/builtin.rs`
  allowlists and fixture manifests are intentionally untouched here).
- `docs/dependency-licenses.json` should be regenerated to list `fancy-regex`
  0.20.0 (MIT), `bit-set` 0.8.0 and `bit-vec` 0.8.0 (MIT/Apache-2.0).
- Possible later extensions, each needing its own .NET proof: case-insensitive
  backreferences, named backreferences `\k<name>`, variable-width look-behind
  with a failing (not truncating) budget.
