# Cardigann XML search responses

`response: {type: xml}` now compiles and extracts through `Plan::extract`.
The pinned `fe8876d2` corpus gains 17 offline plans: 496 → 513 of 583.
Runtime admission remains 340: this change does not alter built-in allowlists or
lifecycle fixture manifests.

## Design and upstream contract

Jackett's `CardigannIndexer.cs:1782–1798` parses the already decoded response with
`new XmlParser()`, optionally reparses after preprocessing, and passes the result
to CSS row selection. The subsequent field extraction and row merging are shared
with HTML. Rackett follows that path using the existing `roxmltree` dependency to
validate XML and build a `scraper::Html` tree directly. It never sends XML through
html5ever: there are no inserted html/head/body nodes, HTML void-element rules,
HTML entity expansion, or HTML raw-text treatment of title/link/template.

`src/cardigann_exec/xml.rs` owns parsing and conversion. The existing CSS engine,
selector mutations, cases, templates, filters, category mapping and release
normalization operate on that same tree. A leading internal processing instruction
identifies XML to the selector wrapper; XML type matching and `:empty` have small
overrides. HTML behavior and JSON parsing retain their existing paths.

The decisions below are based on Jackett's pinned references to AngleSharp 1.8.6
and AngleSharp.Xml 1.2.0, including these implementation sources:

- [XmlParser.ParseDocument(string)](https://github.com/AngleSharp/AngleSharp.Xml/blob/v1.2.0/src/AngleSharp.Xml/Parser/XmlParser.cs)
- [XmlDomBuilder](https://github.com/AngleSharp/AngleSharp.Xml/blob/v1.2.0/src/AngleSharp.Xml/Parser/XmlDomBuilder.cs)
- [XmlDocument.CreateElementFrom](https://github.com/AngleSharp/AngleSharp.Xml/blob/v1.2.0/src/AngleSharp.Xml/Dom/Internal/XmlDocument.cs)
- [XmlElement.TextContent](https://github.com/AngleSharp/AngleSharp.Xml/blob/v1.2.0/src/AngleSharp.Xml/Dom/Internal/XmlElement.cs)
- [XmlCDataSection](https://github.com/AngleSharp/AngleSharp.Xml/blob/v1.2.0/src/AngleSharp.Xml/Dom/Internal/XmlCDataSection.cs)
- [XML entity provider](https://github.com/AngleSharp/AngleSharp.Xml/blob/v1.2.0/src/AngleSharp.Xml/XmlEntityProvider.cs)
- AngleSharp's local checked-out `Css/Dom/Internal/TypeSelector.cs`,
  `FirstTypeSelector.cs`, `TextContainsMatcher.cs`, `Dom/Internal/Element.cs`,
  `NamedNodeMap.cs` and `Css/DefaultPseudoClassSelectorFactory.cs`.

### Names and namespaces

XML preserves element spelling and prefixes (`pubDate`, `animeTitle`, `fileSize`,
`torznab:attr`). AngleSharp's CSS type selector nevertheless compares **local
names** with `OrdinalIgnoreCase`. Rackett matches ASCII names without case
sensitivity, preserving the original qualified name so structural selectors such
as `:nth-of-type` compare it ordinally. Non-ASCII element names are rejected rather
than approximating .NET Unicode case folding; feed text and attribute values may
contain Unicode.

AngleSharp.Xml's `XmlDocument.CreateElementFrom` constructs `XmlElement` with a
null namespace URI even when xmlns declarations are present. Rackett likewise
keeps the prefix/local name and an empty element namespace. Consequently `attr`
and `*|attr` match `torznab:attr`; escaped `torznab\:attr` is a different local name
and does not match. A default namespace does not hide RSS items. Named namespace
selectors such as `torznab|attr` remain outside the existing HTML CSS dialect.

`Element.GetAttribute(name)` looks up the **qualified attribute name** ordinally
on these XML elements. Rackett stores qualified attribute names for both CSS
attribute selectors and field extraction: `Name` differs from `name`, and
`t:Value` differs from `Value`. Namespace declarations are retained as attributes,
including local redundant declarations. Attribute values receive XML entity and
line-ending normalization, then the trim performed by `XmlDomBuilder`.
The pinned Torznab definitions select elements with `[name=seeders]` and read
`value`; the namespace prefix of the element does not affect that selection.
Attribute value comparison retains the existing CSS dialect and its limitations;
this work does not broaden its .NET Unicode case-comparison support.

### Text, CDATA and entities

Field text concatenates descendant ordinary text and CDATA, excluding comments
and processing instructions, then uses the existing whitespace normalization.
CDATA preserves literal markup and entity spellings: `<![CDATA[&amp;]]>` yields
`&amp;`, while ordinary `&amp;` yields `&`. The XML predefined entities and numeric
character references are decoded; HTML-only named entities such as `&nbsp;` fail.

There is an upstream distinction: `XmlCDataSection` implements `ICharacterData`,
not `IText`. `TextContainsMatcher` scans only `IText`, so `:contains` ignores CDATA
but continues its match across ordinary text on either side. `:empty` uses child
element count and `TextContent`, so nonempty CDATA prevents an empty match.

roxmltree normally coalesces adjacent text and CDATA. A bounded lexical pass
therefore replaces CDATA tokens with numbered internal processing instructions,
retaining their literal data separately. It skips comments, PIs and quoted tag
attributes, checks XML character validity, and normalizes CR/CRLF. The XML parser
validates the rewritten document, and conversion verifies that each CDATA belongs
inside an element. This preserves the upstream distinction without quadratic
repeated text concatenation. Source PIs beginning `rackett-` are rejected to keep
internal markers unambiguous.

### Decoding and response handling

The definition charset wins, as described in [legacy-encodings.md](legacy-encodings.md).
The runtime decodes bytes before extraction; the XML parser receives a Rust string.
An XML encoding declaration never changes that string or initiates another decode.
There is no charset, BOM, HTTP-header or XML-declaration sniffing in this path.
A Latin-1 fixture with a conflicting windows-1251 declaration verifies this.

Raw XML must parse before preprocessing; a filter cannot repair a malformed raw
response. With preprocessing, its result is parsed again. Without filters the
first DOM is reused. The XML branch does not run HTML error-block checks, matching
Jackett. Plans combining XML and `search.error` remain conservatively rejected by
the existing compiler guard; none of the 17 new plans needs that combination.
No generic runtime response-type gate needed changing. HTML-only cookie-session
and challenge-solving restrictions remain in place.

## Bounds and unsupported behavior

- Original response and decoded text: existing 8 MiB caps. XML parsing independently
  checks the text cap. The temporary CDATA representation is bounded by 12 MiB and
  100,000 sections, and the parser has a 100,000-node cap.
- The resulting DOM receives the existing 100,000-node/depth-128 validation;
  the internal marker also counts. XML conversion rejects depth above 128.
- Selection retains 1,000 rows, 100,000 visits per pass, 20 million aggregate DOM
  visits, 1 MiB field output, 32 MiB extraction work and `:contains` scan budgets,
  20,000 extraction operations, and 8 MiB release output.
- Malformed XML, undeclared namespace prefixes, invalid characters, DTDs and custom
  entities fail as response errors. DTDs are rejected and external resources are
  never loaded. Parser diagnostics do not echo response contents.
- Non-ASCII element names, reserved `rackett-` PIs, named namespace selectors,
  XML `search.error` plans, and all previously unsupported selector/filter features
  remain unsupported. These limits are explicit rather than HTML fallbacks.

## Corpus and validation

`tests/cardigann_xml.rs::pinned_xml_compilation_delta` asserts **17/18** and prints
both the successful IDs and the exact remaining diagnostic:

anibt, binarybay, bitagent, bitmagnet, bluetorrents, fileiplay, nekobt, pandacd,
periodical, pornotorrent, retrotoon, scenetime-api, shadowbit, showrss, stellarwinds,
torrentclaw, world-of-tomorrow-api.

Remaining:

```text
dasunerwartete-api: $.search.fields._onlyup: unsupported CSS selector "free_button:contains(Only Upload)": invalid :contains argument: argument must be exactly one token
```

That is the preexisting multi-token `:contains` restriction, newly exposed after
XML admission. The definition is not modified and the CSS dialect is not expanded.

Construct unit tests cover name/attribute case, namespace prefixes and default
namespaces, structural selectors, CDATA/text/contains/empty, entities, malformed
documents and byte/node/depth/CDATA limits. Full `Plan::extract` fixtures cover the
unchanged pinned Bitmagnet Torznab definition, an RSS enclosure, conflicting XML
encoding declarations, preprocessing, mutation, cases and root selection.

Validation uses `CARGO_BUILD_JOBS=3`:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --lib
cargo test --locked --test cardigann_execution --test cardigann_selectors --test cardigann_dom --test cardigann
cargo test --locked --test cardigann_xml -- --nocapture
cargo run --locked --quiet --example cardigann_diagnostics
```

All commands passed on 2026-10-29. Library tests: 155 passed, 1 preexisting ignored;
the four required Cardigann integration files: 27 passed; new XML integration
tests: 9 passed. An expanded regression run also passed all 81 tests across those
four files plus `cardigann_config_selectors`, `cardigann_rows_after`,
`cardigann_css_contains` and `cardigann_css_inequality`. The six XML construct tests
are included in the library count. Diagnostics confirmed the 17-plan gain above.
Site lifecycle fixtures and runtime admission are follow-up work for the
fixture/integration agents.
