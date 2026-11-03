# Cardigann model, offline checker and execution plans

The model checker loads all 583 pinned YAML definitions offline. Phase 2 adds a separate bounded, immutable **offline execution plan** API with upstream response goldens. **No Cardigann definition is enabled in the service.** The five existing RSS/Atom, Torznab, UNIT3D, JSON and HTML adapter templates remain separate and unchanged.

## Offline use

```sh
cargo run --locked -- definitions check vendor/jackett/definitions
cargo run --locked -- definitions check vendor/jackett/definitions --json
cargo run --locked -- definitions check path/to/definition.yml --json
```

A directory check examines immediate regular `.yml` and `.yaml` files, in sorted path order. It does not recurse or follow directory entries that are symbolic links. An explicitly named file is read directly. Exit status is zero when at least one definition passes the model checks and all checked definitions pass; invalid inputs or an empty directory produce status 1. Invalid command syntax and directory access failures also fail. JSON output contains ordered per-file results, diagnostics, IDs, required structural features, and a summary. `model_valid` and `execution_supported` are separate: the latter is always false. Every result includes an `execution_unsupported` informational diagnostic. The command never starts the HTTP service, opens tracker URLs, instantiates adapters or reads/writes service configuration. It ignores `RACKETT_DATA_DIR`.

Results are deterministic for unchanged inputs at the same paths. Paths are displayed as supplied; JSON output does not redact user-supplied local file names. Diagnostics have stable codes, severity, a logical path and, when available, one-based line/column locations. Catalog duplicate IDs are errors on both definitions. `replaces` is preserved as tracker ID aliases; it does not implement configuration migration or alias resolution/collision detection.

## Parsing guarantees

`src/cardigann.rs` uses the maintained pure Rust `yaml-rust2` 0.13 event API, with default encoding features disabled. This is a single direct dependency; its locked transitive dependencies are recorded in Cargo.lock. The event API exposes scalar text/style and ordered mapping events before high-level YAML type resolution: [primary API documentation](https://docs.rs/yaml-rust2/0.13.0/yaml_rust2/parser/enum.Event.html), [parser API](https://docs.rs/yaml-rust2/0.13.0/yaml_rust2/parser/struct.Parser.html), [upstream project](https://github.com/Ethiraric/yaml-rust2). The general-purpose YAML map loader is deliberately not used.

- Every mapping, including unsupported structures, is `Vec<(Node, Node)>`. Every sequence is `Vec<Node>`. Ordered repeated keys survive everywhere. `search.fields` never becomes a map or a struct with one slot per field.
- Scalar text and style are retained without implicit number, boolean, null, date, octal or hexadecimal resolution. Unquoted category ID `01` remains `01`. Quoted escapes, literal blocks and folded blocks use YAML-decoded text. This is a semantic AST, not a byte-preserving editor: comments, whitespace, quote spelling, block chomping notation and the optional leading UTF-8 BOM are not round-tripped. Original vendor files remain unchanged.
- `_intermediate` and arbitrary field names, suffixes and exact `|` modifier strings survive. `Field::base_name`, `modifiers` and `intermediate` are views; they do not rewrite names or reorder extraction. Repeated-field evaluation is future interpreter work.
- Exactly one UTF-8 YAML document is accepted. Default limits are 2 MiB of file bytes, nesting depth 64, and 100,000 nodes (mapping keys count). File reads use a bounded reader, including files growing during a read. Configurable depth cannot exceed the absolute ceiling of 128. Oversize/deep/node-heavy inputs return diagnostics without building an unbounded AST.
- Anchors, aliases, explicit tags, YAML directives, merge keys (`<<`) and complex mapping keys are rejected explicitly. Alias expansion is **zero**. No anchors, aliases or explicit tags occur in the pinned corpus, verified by an independent event scan. Tracker aliases in `replaces` are unrelated to YAML aliases. A BOM is stripped before directive checks.
- Syntax, unsupported YAML, invalid UTF-8, I/O and resource-limit failures return diagnostics. No dynamic constructors, filesystem includes or network resolution exist. Unit tests exercise malformed, repeated, deeply nested and limited inputs. No claim of a formal proof against every possible parser defect is made.

## Validation contract

The unchanged pinned upstream `schema.json` supplies model property shapes, required properties, scalar enumerations, references, array item shapes and dynamic mapping value shapes. The validator checks required-property `oneOf`/`anyOf` alternatives (including search path choice, category choice and required download/magnet/infohash fields) and primitive/container union shapes. Nested array union items are checked. Root IDs must be nonempty ASCII letters/digits/hyphen/underscore; mixed-case upstream IDs are preserved. Site links must be nonempty absolute HTTP(S) URLs without credentials. Request delays must be finite and nonnegative. Boolean model values accept the exact text `true`/`false`; scalar integers/numbers are checked only when a model property asks for them. The underlying AST always retains strings.

This is **not a full JSON Schema draft 2019-09 validator**. `model_valid` means these bounded model checks passed, not full schema conformance, tracker availability or executable compatibility. In particular, regex `patternProperties` key patterns, `pattern`, `format`, `uniqueItems`, general numeric/array bounds and conditional `if`/`then`/`allOf` rules are not evaluated. Dynamic mappings validate their shared value schema without regex-matching keys. Primitive scalar alternatives are shape checks rather than YAML implicit typing, so quoted numeric defaults remain text. The checker does not perform execution semantic validation. Separate `cardigann_exec::Plan::compile` validates its supported execution subset; setting-reference existence is checked during template rendering.

Unknown properties emit warnings and retain their raw subtrees. Unknown enumerated values are errors while still remaining in the AST. Repeated properties outside `search.fields` emit ambiguity warnings; first-value typed convenience access is explicit. Repeated search fields are valid and ordered. This permits the next phase to inspect unsupported extensions without discarding them. The C# model contains a `ratio` block absent from its schema; it is retained raw with an unknown-property warning.

## Architecture API

`parse(&str, Limits)` and `load(&Path, Limits)` return an owned `Node` or `Diagnostic`; parsing alone does not imply model validity. `validate(&Node)` returns model diagnostics. `check_path` composes bounded loading, validation, cross-file duplicate-ID checks and a `CheckReport`, with sorted required-feature sets. Errors in one definition do not suppress later results.

`Definition { raw: &node }` exposes borrowing `Metadata`, `Setting`, `Capabilities`, `Login`, `Search` and `Download` views. `Selector`, `Rows`, `Request`, `Filter` and `CategoryMapping` keep their raw nodes available. Settings expose options/defaults; caps expose ordered category mappings and modes. Search fields are an iterator of `Field { name, selector }` retaining repeats. Search requests include both legacy `search.path` and modern `paths[]`; callers should validate first, since the model rejects both together. Login inputs, selector filters/cases and download-before/selector/infohash inputs remain inspectable without cloning or mutation.

`Node::get` returns the **first** property; `Node::all` and `Node::pairs` preserve every occurrence. Interpreter code must iterate `Search::fields`, never collect it into a unique-key map. Typed convenience defaults include `testlinktorrent=true`, `inheritinputs=true`, `queryseparator="&"`, HTML response type, false redirect/optional/multiple flags and rows.after=0. These do not insert default nodes. Invalid typed values must be handled through validation, not silently executed using convenience fallbacks. Raw unknown subtrees and all original scalar IDs are always available. No lifetime-bearing views are stored in global service state.

## Required feature inventory and next phase

`features` and JSON `required_features` inventory every structural property path plus named filters, exact field names/modifiers, encoding/method/type values and Go-template presence. They are **requirements**, with `execution:unsupported`, not implemented feature flags. This is a structural inventory: it does not parse individual Go-template operations, .NET regex constructs, CSS extensions, or JSONPath expressions. Those require semantic analysis and fixtures next.

Required DSL families include metadata/aliases/certificates/encodings; dynamic settings/options/defaults; categories and search modes; Go templates and `.Config`/`.Query`/`.Result` variables; request paths/inputs/raw inputs/headers/inheritance; response HTML/JSON/XML selection; selector attributes/text/cases/defaults/optional/removal; row count/dateheaders/merging/multiple/filters; ordered intermediate and modified release fields; login cookies/forms/CSRF/get-and-selector inputs/captcha/error/test; ratio; download-before/path selectors/fallback selectors/infohash/headers/POST and redirects. None is executed here. The precise observed filter-name inventory is recorded below after running the checker.

Next phase: implement an immutable compiled interpreter plan from validated AST, preserving ordered extraction and per-request variables. Add a fixture for every observed template/filter/selector operation, including bounded regex compatibility and legacy encodings. Port upstream HTML and 78-release JSON response goldens before enabling any definition. Then build isolated session/login/cookie behavior and download resolution with recorded synthetic HTTP transcripts. Required-feature diagnostics must continue to block unsupported execution; no catalog count can establish tracker compatibility.

## Evidence and provenance

`vendor/jackett/NOTICE`, `LICENSE` and `SHA256SUMS` pin all 583 unchanged definitions, schema and two upstream YAML model fixtures to Jackett commit `fe8876d21b93dfda84eb12bd24e1c72e56fb8736`. The existing root GPLv2 license remains unchanged. Tests verify every vendored checksum and parse/check all 583 files. They port the upstream all-definition parser gate and metadata/settings/category/mode assertions from the HTML/JSON fixtures; Phase 2 additionally ports both response tests, described below. Synthetic tests cover duplicate ordered fields, intermediates/modifiers, ID aliases, lexical scalar IDs, unsafe YAML, resource limits, nested model errors, duplicate IDs, deterministic CLI output, failure exits and absence of configuration mutation. No live trackers or parent preview are touched.

Observed named filters (23): `andmatch`, `append`, `base64decode`, `dateparse`, `diacritics`, `fuzzytime`, `htmldecode`, `jsonjoinarray`, `prepend`, `querystring`, `re_replace`, `regexp`, `replace`, `split`, `timeago`, `timeparse`, `tolower`, `toupper`, `trim`, `urldecode`, `urlencode`, `validate`, `validfilename`. The offline engine supports the 22 field-filter names within the constraints below; `andmatch` is a row-filter capability and remains blocked.


## Phase 2: immutable offline execution (2026-10-27)

`src/cardigann_exec/` provides `Plan::compile(&Node) -> Result<Plan, ExecutionError>`, `plan.context(settings, query, injected_utc_clock)`, `plan.build_requests(&Context)`, and `plan.extract(&BuiltRequest, body, &Context) -> Vec<Release>`. Plans own their data, have no mutable shared definition or session state, and are reusable concurrently. `.Result` starts fresh for each row. `ExecutionError` contains a logical source path and can convert to the established `Diagnostic`. Runtime adapters, catalog, credentials and network transport are untouched. `Plan::blockers()` reports login/session/challenge, download-resolution, ratio and custom certificate integration requirements even when offline response parsing succeeds. A compiled offline plan is **not** authorization or proof for network execution.

The separate diagnostic example reports offline compilation versus runtime availability without changing the model checker's `model_valid` or its always-false service `execution_supported`:

```sh
cargo run --locked --example cardigann_diagnostics -- vendor/jackett/definitions
cargo test --locked --test cardigann_execution
```

### Proven behavior

The unchanged upstream response bodies and original C# response assertion tests are copied into `vendor/jackett/fixtures/`, covered by the checksum manifest. Generic execution returns **25 HTML releases and 78 JSON releases**, asserting all upstream first-release normalized fields: title, categories (including custom `100001`), details/download/GUID, magnet/infohash presence, poster, dates, sizes, seeders/total peers, files/grabs, volume factors, minimum seed time, IDs and gain. The HTML relative date uses an injected October 2026 clock, yielding 2024 as upstream expects. No expected releases are precomputed in engine code. A four-thread test reuses one plan and demonstrates isolated ordered intermediates and append fields.

Supported templates: plain variables, `if`/`else`, `range` (including index binding), `and`/`or`, `eq`/`ne`, `join` and `re_replace`. Quoted literals preserve upstream backslashes. Unknown functions, missing variables and unsupported grammar return errors. Context exposes configured/default `.Config`, core `.Query` fields/flags, `.Keywords`, mapped `.Categories`, `.Today.Year`, and request-local `.Result`. Checkbox values use upstream `"True"`/null. Broader query synthesis and settings/session UI integration remain next-phase work. Multi-select defaults are retained as arrays.

Requests retain ordered repeated inputs, path input inheritance, GET deduplication, POST form bodies, category include/exclude routing, templated first header values (matching upstream), literal path syntax with encoded substitutions, and UTF-8 WebUtility-style value encoding. Unsafe literal input keys and header controls reject explicitly. Base URL is the first definition link; alternate configured site links are not yet integrated.

HTML selectors support scraper's CSS syntax, bounded `:contains(...)` (see below), row self-matching, `:root`, attributes and ordered cases. Other unsupported CSS extensions fail validation. JSON selectors support bounded dot/bracket object/index paths and nested `has`, `not`, `contains`, row arrays and nested row attributes. JSON case keys use exact values or `*`. Ordered field chains, arbitrary intermediates, optional failures/defaults, title/description append, category noappend, relative URLs, magnets, first numeric ID groups, peer totals, dates, binary size units, categories and extra metadata populate the existing `Release`/attributes model. JSON `rows.count` is advisory as upstream: a successfully parsed count below one returns no releases; malformed/missing count is ignored. Count filters are explicitly blocked.

All 22 observed field filters have fixtures: `append`, `base64decode`, `dateparse`, `diacritics`, `fuzzytime`, `htmldecode`, `jsonjoinarray`, `prepend`, `querystring`, `re_replace`, `regexp`, `replace`, `split`, `timeago`, `timeparse`, `tolower`, `toupper`, `trim`, `urldecode`, `urlencode`, `validate`, `validfilename`. All 27 upstream field-filter names have fixture assertions, including additional helpers not observed in this catalog subset. Semantics are deliberately constrained: .NET balancing/conditional regexes fail (except hawke-uno's one pattern), and look-around, atomic groups and numbered backreferences run only within the bounds in [dotnet-regex-lookaround.md](dotnet-regex-lookaround.md); special replacement tokens outside supported numeric/named captures, `$$` and `$&` fail. Date layouts accept the invariant .NET/Go subset detailed below; fuzzy/relative grammar accepts documented common English/ISO/RFC/Unix forms. Unknown locales/tokens fail. `jsonjoinarray` accepts object/numeric-index paths only. `split` and `trim` use the first argument character as upstream. Filename filtering follows upstream Linux invalid characters. These are supported subsets, not a claim of complete .NET compatibility.

### Explicit blockers and limits

Compilation/extraction rejects unimplemented search keys, row merging/multiple/dateheaders/filters (including `andmatch`), HTML cross-field node removal, parent-row JSON `..`, general JSONPath, XML, raw request inputs, non-UTF8 encoding, redirect execution, response `noResultsMessage`, unsupported modifiers and unknown semantics. String-derived custom category hashes are blocked; numeric custom category IDs retain upstream `100000 + tracker_id` mapping. Settings of unsupported complex types can fail when rendered rather than being coerced into a fabricated scalar. Session/login/challenge/captcha/FlareSolverr, cookie persistence, download-before/detail resolution, transport certificate policy, tracker aliases/migration and live availability remain unimplemented. Existing five native adapters remain separate.

Response bytes are capped at 8 MiB; JSON parsing retains serde's depth limit; HTML selection rejects more than 100,000 nodes or depth 128. Selectors cap source length 4,096, JSON nesting 16, CSS pseudo nesting 32 and traversals 100,000. At most 1,000 extracted rows, 128 fields and 32 filters per field are admitted. Per extraction, 20,000 field/filter operations, 32 MiB processed field/filter bytes, 1 MiB release strings and 8 MiB aggregate release strings bound repeated work/output. Templates cap source 64 KiB, 2,048 parts, depth 24, 16,384 render operations, range 4,096 and output 1 MiB; contexts cap depth 64, 65,536 nodes and 4 MiB strings. Regex source is capped at 4,096, compiled program/DFA each 1 MiB, replacements 8,192 and input/output 1 MiB. Requests cap paths 64, inputs/headers 256 and constructed output 128 KiB. Errors never include response bodies or context values.

Dependency APIs were checked against primary documentation: [regex builder bounds](https://docs.rs/regex/1/regex/struct.RegexBuilder.html), [base64](https://docs.rs/base64/0.22/base64/), [Unicode normalization](https://docs.rs/unicode-normalization/0.1/unicode_normalization/), [HTML entities](https://docs.rs/html-escape/0.2/html_escape/). Locked license metadata records these maintained pure Rust dependencies.


Initial Phase 2 pinned corpus diagnostic: **583 definitions, 12 offline plans compile, nine of those have explicit session/transport blockers, zero runtime executable**. The semantic expansion below supersedes these initial counts. The other 571 fail on their first unsupported semantic with source path. Recorded first-failure summary is diagnostic coverage, not an operational tracker list. Independent source review identified and corrected aggregate output budgets, checkbox truth values, request encoding, raw regex literals, partial relative-date matching, case whitespace and ignored count filters; cross-field DOM mutation and parent-row selectors were explicitly blocked.


## Semantic expansion: keyword pipelines and invariant dates (2026-10-27)

Source contract: pinned `CardigannIndexer.cs` `PerformQuery` builds `.Query.Keywords`, runs `Search.Keywordsfilters` once, and assigns its result to `.Keywords` before request templates. `applyFilters` templates replacement strings, not regex patterns. The offline plan now compiles ordered `search.keywordsfilters` and preserves `.Query.Keywords` and `.Query.Q` while exposing transformed `.Keywords` to requests, preprocessing and fields. Configured settings and query bindings remain available throughout. `.Keywords` is unavailable inside its own pipeline, matching upstream assignment order; filters do not observe partial results. Existing query synthesis remains the supported core `SearchQuery` subset; this change does not establish complete Torznab query synthesis parity.

`Plan::prepare_context(&Context) -> Result<Context, ExecutionError>` is additive. It returns a prepared clone, always derives filtered keywords from the original `.Query.Keywords`, and can safely be repeated. `build_requests` and `extract` prepare their own clones so existing callers receive the new semantics without an API change. The caller's settings/query/result context is never mutated. The existing `context` method remains infallible and returns original keywords; callers inspecting transformed keywords should use `prepare_context` and handle its error.

`search.preprocessingfilters` runs in order on an owned response string before HTML selection. JSON bypasses it, as the pinned C# JSON branch does. XML remains unsupported. Each pipeline admits at most 32 operations and uses the existing compiled field-filter implementations, 1 MiB per-filter input/output limit, bounded regex compatibility and template rendering. Keyword preparation has its own32 MiB work budget; preprocessing shares the extraction's32 MiB work/20,000-operation budget with fields. Pipeline input and output both count toward work. The unchanged 8 MiB initial response limit still applies, including before preprocessing. No context or response text is added to diagnostics. Base64 decoding rejects invalid UTF-8 deliberately rather than copying .NET's replacement-character decoding.

The former explicit-Go-year gate blocked common, genuinely supported layouts. Date compilation now follows `DateTimeUtil.ParseDateTimeGoLang`: first try an invariant .NET layout when the source layout contains `y`, `h` or `d` ignoring case, then use its exact ordered Go-to-.NET replacement list. The supported custom tokens are `yyyy`, `yy`, `MMMM`, `MMM`, `MM`, `M`, `dddd`, `ddd`, `dd`, `d`, `HH`, `H`, `hh`, `h`, `mm`, `m`, `ss`, `s`, `tt`, `zzz` and `zz`. Literal punctuation, `T`/`Z`, quoted strings, and backslash escapes are supported. Numeric widths and literal spacing are checked before Chrono parsing, so a two-digit token cannot silently accept one digit. English month/day names use invariant parsing; numeric month/day order is prescribed by the layout and never guessed. Input is trimmed only, matching upstream `ParseUtil.NormalizeSpace`; internal whitespace is preserved. Invalid dates, weekday mismatches, leap seconds, offsets beyond 14 hours, invalid 12-hour values and calendar years outside 1–9999 retain the original input unchanged at the `dateparse`/`timeparse` filter boundary, matching C#'s caught `FormatException`. An unsupported layout fails compilation with an explicit diagnostic.

Both calendar defaults and rollover use the injected UTC clock. .NET layouts without a year assume the current injected year and may produce future dates: `htt MMM. d` (1337x) is intentionally different from the Go fallback. Only a successful Go-converted fallback whose converted format lacks the literal substring `yy` moves a future date back one year (even quoted `yy` suppresses rollback, as in the source); February 29 clamps to February 28 as `AddYears(-1)` does. Time-only layouts use the injected date, omitted time components default to zero, and 12-hour formats without `tt` default to AM. Two-digit years use the .NET 8/9 invariant Gregorian 2049 cutoff (00–49 → 2000–2049, 50–99 → 1950–1999), rather than Chrono's different pivot. Explicit offsets normalize to UTC; absent offsets assume UTC. This deliberately removes the upstream process-local timezone and wall-clock dependency and should be retained when integrating the runtime.

Unsupported date syntax remains blocked: single-letter standard formats, era/calendar and named timezone specifiers (`K`, `MST`), percent specifiers, fractional-second tokens and layouts requiring other locales. Fractional Go layouts previously passed the syntactic gate through a conversion that differed from the pinned source; they now reject explicitly pending faithful fractional-token fixtures. These restrictions are intentional supported-subset boundaries, not broad compatibility claims. Ordinals are not guessed: tests use 1337x's actual preceding suffix-removal regex, and quoted ordinal literals are parsed exactly. Unsupported .NET regex features remain blocked in every pipeline; look-around, atomic groups and numbered backreferences are supported only as described in [dotnet-regex-lookaround.md](dotnet-regex-lookaround.md). String category hashes/collision continuity and row `andmatch` are deferred; existing numeric category IDs and hierarchies are unchanged.

New fixtures exercise settings-dependent ordered keyword replacements, original versus transformed keywords in requests and fields, idempotent context preparation, pipeline assignment timing, HTML-only preprocessing, invalid UTF-8, expansion/aggregate/count budgets, .NET/Go year rollover differences, invariant calendar bounds, offsets, exact widths, ordinals and escaped/quoted literals. Synthetic responses in `tests/fixtures/cardigann-semantics/` run against the unchanged pinned MVGroup Main and OneJAV definitions, asserting requests and normalized releases without network access. The original 25 HTML/78 JSON golden files and SHA256 manifest remain unchanged. An independent source review identified fixes for internal whitespace, single-letter standard layouts, offset conversion, calendar bounds and missing AM defaults; these fixes are covered by regression tests.

Updated pinned corpus diagnostic: **583 model-valid definitions, 40 offline plans compile (previously 12), 31 of those have explicit integration blockers (previously 9), runtime_enabled remains unclaimed 0**. Offline compilation proves syntax admission only; it does not prove session/runtime usability or live responses. Model-check `execution_supported` remains always false. Recorded first-failure summary contains the exact remaining paths; no service catalog activation changed.

Validation: `cargo fmt --all --check`, warning-denying `cargo clippy --locked --all-targets -- -D warnings`, and all **87 tests** in `cargo test --locked` pass. Tests use `LD_LIBRARY_PATH=/usr/lib/rustlib/x86_64-unknown-linux-gnu/lib` for the installed rustdoc workaround. The vendored SHA256 manifest passes unchanged. No dependency, lockfile, license metadata, runtime API, web, deployment or service configuration changed.

## Phase 3: embedded runtime and stock configuration

The executable now bundles all **583** unchanged pinned definitions using a deterministic, sorted generated lookup. `/api/catalog` reports source identity, language/type, actual mode parameters, a safe settings schema, pinned upstream commit and individual SHA-256 provenance. The checksum test verifies embedded bytes against `vendor/jackett/SHA256SUMS`. No source tree or upstream temporary directory is opened at runtime.

Exactly **acgrip**, **btetree**, **milkie**, **uindex**, and **zamundalife** are currently supported runtime presets, backed by loopback HTTP fixtures. These are implementation claims, not live availability checks. The remaining **578** definitions are blocked with their first unsupported semantic or integration reason. All five custom adapter templates remain available. New installations have zero configured sources.

`kind=cardigann` persists a private `definition` envelope: `{"builtin_id":"milkie","settings":{"apikey":"…"}}`. The URL is an explicit editable base/mirror. The wrapper clones the bounded AST and replaces its first link before compiling; the immutable interpreter APIs are unchanged. One `Arc<Plan>` is reused per configured runtime, while query contexts and parsing state remain local to each call. Server-side compilation, blockers, UTF-8, settings validation and an explicit fixture-backed ID allowlist gate creation, edits and restart. Later interpreter improvements cannot automatically enable untested definitions. Generic cookie/API-key fields are rejected for stock presets; use their schema settings. Text/password settings are treated conservatively as secrets: reads omit their values, and absent/blank edits retain saved secrets only for the same built-in identity and valid HTTP(S) origin (scheme, host and effective port). A URL-only move to another origin removes implicit saved secrets; explicitly supplied nonempty replacement secrets are accepted. API PATCH reads, merges and saves under one writer lock, so queued edits use the latest saved source. Removing the source removes its stored settings; an explicit secret-clear operation is not provided. Files remain private plaintext, not encrypted storage.

The UI makes supported choices easy to select; blocked entries require an explicit browse toggle and cannot be selected. It shows source language/type, mode parameters and supported settings. Informational upstream HTML is displayed as text. Dynamic text/password/select/checkbox/multi-select forms are supported; only the five listed presets have end-to-end stock coverage, and their only credential setting is Milkie's API key.

Search batches validate every HTTP(S) target and header before sending. Targets and redirects must retain the configured origin and cannot contain userinfo. Hop-by-hop/Host/framing headers and control characters are rejected. Milkie's API key is used only in server-side auth/request interpolation and download targets; client URLs use opaque tickets and GUIDs. Stock searches have **zero automatic retries**, including POST failures, regardless of the generic retry field. Up to 64 paths execute sequentially with the larger of pinned requestDelay and user delay, sharing the engine's eight-request admission and queue-inclusive deadline. Responses are capped at 8 MiB and UTF-8; extraction runs in at most eight blocking jobs. Dropping a timed-out query cannot stop an already-running bounded parser or a remote request; the parser keeps its permit until it finishes.

Result categories, bounded pagination, cache coalescing and replacement-runtime invalidation use the existing engine. Plain BTIH magnets are promoted into the download field. HTTP downloads use an existing opaque ticket and a single same-origin GET; detail/download resolution, session/login/challenge/FlareSolverr, custom certificates and ratio-session behavior remain blocked. Download factor/seed-time metadata is descriptive, not ratio enforcement.

Valid empty JSON arrays/counts succeed; malformed JSON/schema and missing required row fields fail. For the two stock HTML definitions, an empty result must retain its expected result container (`tbody` or `table.sr-table`); a login/challenge page without it fails. An unrelated response reproducing that container is not a reliable challenge detector. General HTML challenge recognition is not implemented. No live trackers or credentials are needed by these tests.
## Semantic expansion: raw inputs, response DOM mutation and row matching (2026-10-27)

Pinned source evidence: `CardigannIndexer.cs:1548–1604` builds a `List<KeyValuePair<string,string>>`, preserving repeated inputs and inherited-before-path input order. For `$raw`, template substitutions alone use `WebUtility.UrlEncode`, then the rendered string splits on `&` and the first `=`; empty keys are skipped, missing/empty values retained regardless of `allowEmptyInputs`. `StringUtil.cs:155–158` leaves GET keys literal and encodes values a second time. Thus a substituted ampersand becomes `%2526`, while a literal ampersand separates inputs; substituted spaces become `%2B`. Literal percent signs in raw values receive one final encoding pass. UTF-8 POST uses `FormUrlEncodedContent` (`Utils/Clients/WebClient.cs:242–247`), escaping both keys and values with RFC3986 form rules; GET retains WebUtility's distinct safe characters. Literal path queries still receive upstream's second `?`. These differences now have byte-exact regressions, including brackets, dollars, Unicode, repeated keys, empties, ranges and conditionals.

Expanded inputs share the 256-input limit; rendered and final encoded data retain 128 KiB limits. Raw keys containing whitespace, controls, `#` or `?` explicitly reject, rather than allowing literal syntax to truncate/change the request. Ordinary literal input keys retain a conservative safe-character gate (now including `$`). URL scheme/userinfo and header control checks remain enforced. This is deliberately a bounded UTF-8 subset; arbitrary Go grammar, alternate encodings and unsafe literal keys do not become supported through `$raw`. Runtime errors retain inherited or path-local input source paths.

`CardigannIndexer.cs:1290–1369` selects an element, removes matching **descendants**, then reads cases, attributes or text. Text selectors bypass removal. The extraction engine now owns one mutable parsed HTML tree per response and snapshots row slots before field execution (`:1832–1854`). Removal persists for later fields, including when an optional attribute/case fails; sibling selectors observe detachment. Captured nested rows remain available after detachment, and `:root` follows each node's current ancestor chain. Plans and concurrent extractions share no DOM. No table fragment serialization/reparse is used. Standalone `Selector::extract_html` makes an owned tree copy when removal is requested; `Plan::extract` shares its response-owned tree across ordered fields/rows. Bare `:root` on a non-root row fails explicitly after ancestor-prefix expansion, matching upstream's empty-selector failure; unknown CSS extensions remain blocked.

The existing 100,000-node/depth-128 document gate, per-selector visits and output bounds remain. An additional 20,000,000 aggregate HTML visit budget covers row-slot lookup, selection, removal and case/text traversals, including rendered root selectors; exhausting it is a response error even in optional fields. This conservative accounting can reject very large legitimate documents before the row limit. Field/filter operation, work and output budgets remain independent and cumulative.

`search.rows.filters` now admits only `andmatch` with an optional signed 32-bit scalar character limit (`CardigannIndexer.cs:2435`, `TorznabQuery.cs:217`). Matching runs on final titles after all fields, uses transformed `.Keywords`, falls back to a locally sanitized original query and supported season/episode synthesis when transformed keywords are blank, and preserves the first filter's cached token limit. Limits count UTF-16 code units; word splitting follows .NET's BMP letter/nonspacing-mark/decimal-digit/connector categories. Tokens of length one and `and`, `the`, `an` are ignored. All remaining tokens must occur as substrings. Capability-dependent ID bypass follows the actual C# checks, including its IMDb-dependent Douban/Trakt/TVMaze/TVRage behavior. No raw row logging is added: `strdump` and unknown row filters explicitly fail compilation.

Unicode matching supports canonical decomposition, case normalization and Greek final sigma. Full .NET invariant linguistic collation is **not** implemented: format/ignorable characters and known expansion cases (`ß`, `ẞ`, `Æ/æ`, `Œ/œ`, `Ĳ/ĳ`, `ſ`, `Ø/ø`, `Ł/ł`, `Đ/đ`, `Ħ/ħ`, `Ð/ð`, and U+FB00–FB06 ligatures) return explicit extraction errors rather than approximate matches. Other platform collation differences remain a compatibility risk. Grouped/decimal/non-ASCII numeric episode coercion and negative seasons in blank-keyword fallback reject explicitly. ASCII digit extraction follows ParseUtil (for example `01A` → `01`, `Special` → `00`, `-2` → `02`). Queries cap at 1 MiB and 4,096 tokens, with matching charged against the extraction work/operation budget. Public `.Query.Keywords` and `.Query.Q` bindings remain unchanged. String category hashing/collision handling remains deferred.

Source-referenced synthetic tests now include unchanged pinned Azusa (raw category inputs and `<tr>/<td>` description removal, retaining login blockers) and VSTorrent (row matching, retaining download-resolution blockers), alongside negative/budget, nested detachment, optional failure and concurrent plan regressions. The existing 25 HTML/78 JSON goldens and vendor checksum manifest remain unchanged. Independent source review corrected POST key/value encoding, rendered-root traversal accounting, UTF-16 token categories and sanitized fallback query handling.

Updated offline diagnostic: **583 model-valid definitions; 289 offline plans compile; 275 compiled plans have explicit integration blockers; 294 definitions fail at their first unsupported semantic**. The exact first-failure path counts are recorded in `cardigann-execution-summary.json`. **runtime_enabled remains unclaimed 0 in this isolated semantic work**. Offline syntax admission does not establish fixture-backed runtime readiness or live availability. No catalog activation, login/session/captcha/FlareSolverr, download resolution, deployment or credentials changed. No dependencies or vendor bytes changed. Prior sections' raw-input/removal/row-filter blockers are superseded only within the bounds above.

Validation for this expansion: all **103 tests** pass with `LD_LIBRARY_PATH=/usr/lib/rustlib/x86_64-unknown-linux-gnu/lib cargo test --locked`; `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings` pass. All **591** vendored manifest entries match their unchanged SHA256 checksums. Existing public Plan/context/build_requests/extract APIs remain unchanged; helper additions do not require runtime caller changes.

## Category IDs, collisions and normalization

Category execution now follows pinned `TorznabCapabilitiesCategories.cs:53–104`, `ExpandTorznabQueryCategories` and `CardigannIndexer.cs:1493,2265–2300`. The focused tests port the mixed numeric/string dataset and query/normalization expectations from `Jackett.Test/TestHelpers/TestCategories.cs` and `Common/Models/TorznabCapabilitiesCategoriesTests.cs`. Definitions and original 25 HTML/78 JSON response goldens are unchanged.

A mapping with no description adds only its standard category. A present description, including an empty description, adds a custom category: parse an ASCII signed decimal as a C# signed 32-bit integer (allowing leading/trailing ASCII whitespace), otherwise hash the **original UTF-8 ID** using SHA-1 and read the first two bytes as a little-endian unsigned 16-bit integer, then add 100000. This matches the pinned little-endian host behavior and upstream `con_wii → 137107` assertion. Rust's hash functions are not used. Values outside `int.TryParse` range hash as strings; e.g. `2147483648 → 134421`. The new `sha1` dependency is used solely for compatibility IDs.

There is no collision repair or sequential reassignment. `collision_497` and `collision_511` both have SHA-1 prefix `4d b7`, yielding custom ID `146925`; numeric tracker ID `46925` yields that same ID. Querying it returns all three tracker IDs in definition order. Query results use case-sensitive distinct tracker IDs. Parent queries include real mapped children, child queries do not implicitly include parents, and unknown custom IDs are roots rather than arithmetic thousand-group children. Signed numeric IDs can collide with standard IDs; tests cover real child `2030` versus unknown root `2099`. No optional child-to-parent query expansion is enabled for Cardigann.

Result ID/description matching ignores case, excludes whitespace-only values, and does not trim nonblank values. Upstream `ReleaseInfo.Category` starts as an empty non-null list, so ordinary fields apply ordered `Union` from the first field and remove duplicates. Explicit `noappend` assigns the raw mapping list, retaining duplicate standard/custom IDs; a subsequent ordinary field deduplicates again. Unknown values leave prior categories unchanged. Default query categories preserve their source order and duplicates when no requested mapping matches. The existing `supported_categories()` API still returns sorted unique directly mapped IDs below 100000; it does not expose a new category-tree API.

Bounds are explicit: at most **4096 mapping entries**, **4096 UTF-8 bytes per ID or description**, and **1 MiB combined ID/description bytes**. NUL in category text rejects. Custom numeric addition must produce a nonnegative signed 32-bit result (`0..=2147483647`); negative results and signed overflow reject rather than fabricate unsigned IDs or mirror unchecked C# overflow. Category comparisons consume one extraction operation per mapping and comparison input bytes from the existing 20000-operation/32 MiB work budget; optional fields cannot swallow budget exhaustion. Query membership uses a set rather than scanning every query value for each mapping.

The supported invariant-comparison subset covers canonical decomposition, case normalization and Greek final sigma, retaining accents (`PELÍCULAS` and decomposed `Películas` match `Películas`; `Peliculas` does not). Full .NET invariant linguistic collation remains unimplemented. Known expansions/ignorables (`Cf`, `ß/ẞ`, `Æ/æ`, `Œ/œ`, `Ĳ/ĳ`, `ſ`, `Ø/ø`, `Ł/ł`, `Đ/đ`, `Ħ/ħ`, `Ð/ð`, U+FB00–FB06) reject during nonblank comparison, including exact matches. Other platform collation differences remain a compatibility risk. Unicode hashing itself uses exact UTF-8 bytes. These limits supersede earlier statements that all string hashes are deferred; other unsupported semantics remain bounded as previously documented.

Reproduce the offline corpus report from the repository root:

```sh
LD_LIBRARY_PATH=/usr/lib/rustlib/x86_64-unknown-linux-gnu/lib TMPDIR=/dev/shm \
  cargo run --locked --quiet --example cardigann_diagnostics > /tmp/rackett-category-corpus.json
```

The pinned 583 definitions now produce **316 offline plans** (previously 289), **291 plans with explicit integration blockers** (previously 275), and **267 first semantic failures** (previously 294). The 27 newly compiling IDs, exact first-failure paths and limits are recorded in `cardigann-execution-summary.json`. The 37 previous first category-hash failures are cleared; ten reveal subsequent unsupported semantics. The same fixture-backed runtime allowlist remains **acgrip, milkie, uindex**. Compilation is syntax admission, not runtime or live-availability evidence; no stock source was enabled and no live tracker was contacted.

Independent source review corrected first-field union behavior after checking the `ReleaseInfo` initializer and removed an exact-equality bypass of unsupported Unicode checks. Focused regressions cover both corrections, integer/string/mixed mappings, empty/missing descriptions, leading-zero/sign/whitespace lexemes, SHA-1 byte order, collisions, defaults, hierarchy, duplicate and `noappend` behavior, Unicode bounds and budget exhaustion.

Validation in this isolated checkout: `cargo test --locked --quiet` passed **125 tests**; the nine category tests passed again after strengthening the optional-budget regression. `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `git diff --check` and `sha256sum --check --quiet SHA256SUMS` from `vendor/jackett` passed. Commands requiring Rust shared libraries used the `LD_LIBRARY_PATH` above. Two offline corpus runs produced identical reports. This is local fixture/source validation, with no live availability claim.
See stock preset expansion evidence and candidate audit for the additional fixture-backed preset. Offline plan counts are unchanged; four IDs are explicitly enabled, with no live availability claim.

Parent integration retains the category expansion: **316 offline plans, 291 integration-blocked plans, four fixture-backed enabled IDs (acgrip, btetree, milkie, uindex)**. BT.etree activation does not change offline compilation counts.

## Bounded response row selectors

The pinned corpus's 87 first failures at `$.search.rows.selector` were **69 templated-row rejections, 16 unsupported CSS selectors and two JSON `$` roots**. These were actual row-selector diagnostics, rather than downstream response-type failures. The bounded expansion addresses literal configuration conditionals and the exact JSON root token. At that checkpoint, CSS extensions such as `:contains(...)` and `[attribute!=value]` remained explicit errors; the later `:contains` expansion is recorded below.

`CardigannIndexer.cs:326,438–475,1682,1832,2491–2568` at pinned commit `fe8876d21b93dfda84eb12bd24e1c72e56fb8736` supplies the source behavior. HTML rows render with the search variables; JSON rows render using base variables. The admitted common subset uses only bare `.Config.*` variables in flat, single-line `if/else` blocks with literal arms. Interpolation, ranges, nested blocks, omitted `else`, parenthesized conditions, multiline arms and nonconfiguration conditions reject. The raw condition is checked before the shared expression parser normalizes parentheses. Every combination of branches is validated against the response format during compilation, including branches hidden by default settings. At most **64 variants** of **4096 UTF-8 bytes** each are admitted; empty branches reject. Unsupported syntax cannot become usable by toggling a setting. The compiled template renders separately for each extraction without shared response/query state.

The exact JSONPath `$` selects the token passed to Newtonsoft's helper. For rows, that token must resolve to one array; objects, missing arrays and excessive row counts fail. Compound rooted paths, recursive descent, wildcards and query expressions remain unsupported. This is not a general JSONPath implementation. Existing `has`, `not` and `contains` filters continue to execute on each candidate row before any row attribute selection.

Scalar floating-point conversion now follows the bounded ordinary-decimal portion of `JValue.ToString()`/scalar string conversion: **0.0 → `0`, 1.0 → `1`, 0.5 → `0.5`**. This matters for NexusPHP API freeleech filters that reject multipliers containing a decimal point. The supported numeric subset is zero or `0.0001 <= abs(value) < 1e15`, with at most **15 significant decimal digits**. Values needing exponent or higher-precision compatibility reject explicitly. Such errors propagate through nested `has`/`not` and row selection, instead of silently becoming missing rows. The existing serde_json dependency enables `float_roundtrip` so raw decimal inputs are correctly rounded before this precision gate; no dependency version or lockfile change is needed. Full .NET number formatting is not claimed.

`tests/cardigann_selectors.rs` executes unchanged **13City API**, **Order66**, and **Zamunda LIFE** definitions against synthetic responses derived from their pinned selectors and response comments. Tests exercise true/false freeleech branches, decimal-zero multipliers, category/date/size/peer normalization, root arrays, concurrent contexts, all-branch rejection, unsupported numeric formatting, variant/length limits and excessive row traversal. These are offline synthetic fixtures, with no live tracker availability claim. The original **25 HTML/78 JSON goldens**, vendor bytes, raw input ordering, response-owned DOM mutations, traversal budgets, rowfilters, category hashing/collisions and `supported_categories()` API remain unchanged.

The resulting pinned corpus has **583 definitions, 362 compiling offline plans (+46), 336 plans with integration blockers, and 221 first semantic failures**. Row-selector first failures fall to **22**: 20 unsupported CSS cases and two templates outside the admitted literal-arm subset. The remaining first request failures are **37 encodings, six redirect policies, one method, three no-results-message policies and 17 response types**. Integration blockers occur in **317 session/login/challenge plans, 39 download-resolution plans and five certificate-policy plans** (these overlap). The exact aggregate paths, 46 newly compiling IDs and remaining row-selector diagnostics are recorded in `cardigann-execution-summary.json`. Reproduce with the existing `cardigann_diagnostics` example and required Rust environment shown above.

At the isolated selector checkpoint, runtime activation remained exactly **acgrip, btetree, milkie and uindex**. Compilation and these synthetic interpreter fixtures do not authorize runtime activation. No runtime/auth/builtin files, credentials, transport, installed binaries, services, deployment or remote state were changed.

Independent source reviews identified the parenthesized-condition bypass, decimal-zero conversion and incorrectly rounded raw JSON inputs; the implementation and targeted regressions address all three. The JSON count early return also precedes row-template rendering, with an explicit timing regression. Final validation passes **135 tests**, `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`, `git diff --check`, and all **591** unchanged vendor checksums. Rust commands use `LD_LIBRARY_PATH=/usr/lib/rustlib/x86_64-unknown-linux-gnu/lib TMPDIR=/dev/shm`. The freshly reproduced corpus report's SHA-256 is recorded in the summary. No dependency version or public interpreter API changed.

The third independent review reported no remaining findings and reproduced all eight focused tests, the full 135-test suite and corpus counts. Its first full-suite attempt hit the unchanged `builtin_runtime::tests::sequential_paths_space_requests_and_post_is_not_retried` 55 ms spacing assertion; both its isolated rerun and full-suite retry passed. The implementation's final full run passed directly. This timing-test flake remains outside the interpreter scope; the runtime file was not edited.


## Parent integration: Zamunda LIFE

The runtime allowlist now contains **acgrip, btetree, milkie, uindex and zamundalife**. Zamunda LIFE uses the unchanged pinned JSON definition with the exact `$` root array, literal API path and limit, keyword replacement, string category mapping and direct magnet links. No authentication or download resolution is required. Its loopback fixture covers create/restart/test/search, query encoding, capabilities/categories, result fields, valid empty arrays, malformed responses, safe HTTP errors and explicit challenge rejection. Independent source review found no remaining findings and confirmed byte identity with the pinned upstream definition. This is fixture-backed support, not a live availability claim. The magnet assertion is at the engine result; existing protocol logic passes magnets through without tickets.

Cookie authentication remains an inactive foundation. Milkie now rejects a missing or whitespace-only API key before making a request; saved sources remain editable, and generic runtimes without a builtin ID are unaffected.


### Ticket identity after source edits

Each runtime carries a process-unique, nonzero identity and its own source
configuration snapshot. Releases keep that identity internally through caching;
it is neither serialized nor accepted from API input. Opaque tickets capture it.
An edited, deleted or recreated source rejects old tickets with HTTP 404 and
`Download expired; search again`, including tickets minted from searches that
finish after an edit. Same-runtime cache hits remain usable; magnets pass through.

Downloads revalidate after waiting for admission and the rate gate. The final
identity check is the admission point: requests admitted before an edit can
finish with that source snapshot, including socket activity that starts just
after the check. This does not claim cancellation of admitted HTTP operations.
No runtime-map lock is held across network I/O. Source cookies/Bearer credentials,
same-origin checks, torrent validation and safe error mapping remain in effect.

## Bounded CSS text matching

The source-reviewed `:contains(...)` extension accepts exactly one CSS identifier
or quoted string, including escaped tokens and surrounding comments. Function
names are ASCII case-insensitive; text matching is exact and case-sensitive,
without whitespace normalization. Descendant text joins across element boundaries;
comments and inert HTML template content are excluded, while script/style text
and foreign-namespace template text are included. Empty needles match. Multiple
arguments, numeric tokens and unterminated strings fail validation, including
inside forgiving selector lists. This single-token subset is deliberately narrower
than AngleSharp's last-token behavior.

Each selection, removal or case pass has a 32 MiB budget charged for visited
nodes and scanned text bytes. KMP matching does not allocate a concatenated
text buffer. Existing selector length/nesting, response depth/node and aggregate
visit limits remain. There is no separate response-wide text-byte budget; total
work also depends on the number of bounded passes. Exhaustion propagates as a
response error, including from optional fields, rather than silently changing
release metadata such as freeleech factors.

Cardigann HTML now uses html5ever with scripting disabled, matching Jackett's
parser setting. HTML `noscript` content is parsed as markup: entity references are
decoded, element attributes are not text, and non-head content in a head noscript
can move into the body. This does not strip content or classify challenge pages.
A normal analytics noscript no longer disables `:contains` on the whole document.

A parser event that drops a declarative shadow-root template and leaks its
contents still marks the document. Nonempty `:contains` rejects that document;
empty needles and ordinary selectors retain their existing behavior. The precheck
runs when an element is matched; an empty removal candidate set can skip it.
Ordinary selection and field text can still traverse inert template contents,
and retained shadow-root templates can affect structural selectors. These remain
DOM-parity limits. The five runtime presets are unchanged, and synthetic fixtures
do not establish live availability or valid-empty/challenge-page recognition.

Parent reproduction at `f935a46` reports **583 definitions, 436 offline plans
(+74), 406 plans with integration blockers, five runtime presets and 147 first
failures**. Row-selector first failures fall from 22 to six: two nonliteral
templates, two attribute-inequality selectors, one malformed escape and rutor's
nested `:has` restriction (newly reached after its earlier field failure clears).
Integration blocker occurrences are 379 session/login/challenge, 50 download
resolution and 11 certificate policy, with overlap. The exact new IDs, failure
paths and report digest are in `cardigann-execution-summary.json`. All 179
combined tests, formatting, Clippy, JavaScript syntax and the optimized build
passed; all 591 vendor checksums and original 25 HTML/78 JSON goldens remain
unchanged.

## Bounded CSS attribute inequality

`[name!=value]` is translated with a CSS token walk into
`:not([name="value" s])` only for the verified ordinal subset. A missing
attribute matches inequality; an empty attribute differs from a missing one
when compared with an empty string. Names and values are re-escaped by the CSS
parser. Strings, comments and `:contains` arguments are never text-replaced.
Supported contexts are the selector root and `:not`, `:is`, `:where`, `:has`.

AngleSharp's default comparison is **not always ordinal**: 46 HTML4 attribute
names, including lowercase `type`, `lang` and `rel`, select .NET case folding.
The source set lookup is case-sensitive on the written name. These comparisons
are rejected unless the supported `s`/`S` flag makes them ordinal; `i` is
unsupported. Uppercase `TYPE` does not select the default folding rule. The
emitted `s` prevents scraper's own HTML attribute rules from changing the
comparison. This is a bounded subset of pinned AngleSharp semantics.

Names must be plain identifiers without namespaces or qualified-name syntax;
`xmlns` is rejected. Values may be an identifier, quoted string or digit-only
sequence. Unsupported numeric forms, spaced/comment-separated `! =`, extra
tokens, divergent tokenizer forms (`--` identifiers, escaped newlines, raw
NUL/CR/FF), other function contexts and malformed operators fail explicitly
before forgiving selector lists can discard them. Original and normalized
selectors must meet the existing length/nesting limits. The token walk counts
brackets/braces as well as parentheses, so sources containing `!` can be
conservatively rejected at the boundary even when no rewrite is needed.

The unchanged pinned 3dtorrents and carpathians definitions have synthetic
full-Plan fixtures for excluded tables, missing attributes and both freeleech
branches. They retain their session/login blockers and are not enabled.

Parent reproduction at `d492bc2` reports **583 definitions, 438 offline plans,
408 integration-blocked, five runtime presets and 145 first failures**. Exactly
the 3dtorrents and carpathians report entries changed from the previous 436-plan
checkpoint; all other results are identical. Four row-selector first failures
remain: 1337x, rutor, tasmanit and torrentgalaxyclone. Historical expansion
diagnostics are labeled separately from the current top-level summary.


## Bounded ISO-8859-1 requests and responses

The definition label `iso-8859-1` (ASCII case-insensitive) now compiles as exact
Latin-1. GET input values encode U+0000 through U+00FF to identical bytes before
WebUtility percent encoding. Unrepresentable text fails before network I/O;
replacement and best-fit behavior are unsupported. Paths and raw substitutions
retain upstream UTF-8 encoding, including double encoding of substituted raw
values. Literal raw values use the definition encoding. Other encoding aliases,
non-scalar labels, non-UTF-8 POST and encoding-dependent `urlencode`/`urldecode`
filters explicitly reject. `querystring` and base64 remain UTF-8 as upstream.

Preparatory transport support decodes Latin-1 bytes to identical code points,
ignoring response charset headers, BOMs and meta tags. UTF-8 retains its existing
strict header and byte validation. Raw responses remain bounded at 8 MiB; the
expanded string must also satisfy the existing 8 MiB extraction limit. Expansion
can allocate up to 16 MiB before that check, in addition to the raw byte buffer;
an earlier decoded-size check remains a prerequisite to reconsider before runtime
activation. Builtin transcoding eligibility, cookie encoding rejection and the
five ready IDs are unchanged. This is offline and loopback evidence only.

Source references at `fe8876d`: `CardigannIndexer.cs:96–126,1193–1202,1543–1591`,
`StringUtil.cs:156–158`, `WebUtilityHelpers.cs:8–17` and `WebResult.cs:18–63`.
The independent source review of candidate `d959afe` found no blockers; its
conservative rejection of unsupported labels and nonrepresentable text is
intentional. Newly compilable plans still require separate integration evidence.

Parent reproduction at `77ed763` yields **583 definitions, 443 offline plans,
413 integration-blocked plans, five runtime presets and 140 first failures**.
The five new plans are deildu, karagarga, newheaven, thenewheaven and tribalmixes;
all retain login blockers and fail builtin transcoding eligibility. Aftershock
moves from an encoding error to its unsupported `urldecode` filter. No other
definition changes outcome; encoding-first failures fall from 37 to 31.


## Bounded HTML source-error checks

HTML plans may now declare one to eight `search.error` blocks. Each accepts a
literal bounded CSS selector and an optional literal `message.text`, which is
validated and discarded. Unknown keys, duplicate keys, templates, unsupported
message forms and plans with any non-HTML response path reject at compile time.
Error blocks inspect raw HTML before preprocessing, in definition order. The
first match returns a static error at `$.search.error[i]`, without page text,
selector text or the definition's message. Matching includes the document root
and skips inert template fragments. Existing node, depth and byte bounds apply;
each block has a separate 32 MiB text-scan budget (at most eight passes).

For error-check plans only, known parser differences cause explicit response
errors before any selector runs: dropped or retained HTML declarative-shadow-root
templates and HTML select elements. This includes filter forms that upstream may
accept. Error elements inside noscript now participate in ordinary matching. These limits are
conservative compatibility boundaries, not full DOM parity or live readiness.
Ordinary inert templates and foreign elements retain their supported behavior;
plans without error blocks retain their existing parsing path.

Source: pinned `CardigannIndexer.cs:538–561,1816–1826` and the AngleSharp/html5ever
parsing differences recorded in the two independent source reviews. The first
review found missed-error paths; the second accepted the final guards in
`32b37f3`. The stack is integrated through `ae90577`. Runtime allowlist,
HTTP status policy, cookie/login gates and JSON empty-response behavior are
unchanged by this slice.

Parent reproduction at `ae90577` yields **583 definitions, 452 offline plans,
420 integration-blocked plans, five runtime presets and 131 first failures**.
Nine definitions gain offline plans; 52bt and magnetcat have no integration
blockers but remain inactive. Seven retain login/certificate blockers. The other
three former search-error failures now expose regex or rows-after limitations.
These counts do not establish whether real pages satisfy the document guards.


## Exact JSON empty-response policy

JSON search paths may declare `response.noResultsMessage` as a quoted empty
string. Other values, non-JSON paths and unsupported response keys reject.
The flag belongs to each built request and is not inherited by other paths.
Policy-bearing definitions reject repeated structural properties in root,
search, path and response mappings, even when a later occurrence hides the
policy. Repeated input parameters retain their existing ordered behavior.

`Plan::extract` remains available but refuses requests carrying this policy
because it lacks transport facts. `Plan::extract_response` receives actual final
status, whether any redirect was followed, final Refresh-header presence and
raw body length. Policy requests require exactly HTTP 200 with no redirect or
Refresh, for empty and nonempty bodies alike. Only zero raw bytes and an empty
decoded string skip that path. Whitespace, BOMs and malformed JSON do not count
as empty. Other paths continue contributing results, and ordinary requests retain
their previous behavior. Metadata is a trusted programmatic interface; production
constructs it from the HTTP response, not source settings or query input.

Upstream references at `fe8876d`: `CardigannIndexer.cs:951–975,1608–1645` and
`WebResult.cs:55–74`. Two independent reviews checked transport fidelity and
structural ambiguity; the final stack is integrated through `d68223f`.
The three newly compilable definitions retain login blockers. Cookie handling,
runtime eligibility and the five ready IDs remain unchanged.

Parent reproduction at `d68223f` yields **583 definitions, 455 offline plans,
423 integration-blocked plans, five runtime presets and 128 first failures**.
Only czteam-api, digitalcore-api and superbits change, each to an offline plan
with a login blocker. No remaining first failure uses `noResultsMessage`;
`missingAttributeEqualsNoResults` and all unrelated policies remain blocked.

## Exact bounded .NET Unicode blocks

Definition-sourced `regexp`, `re_replace` and template `re_replace` compile
through a shared property translator. The exact, case-sensitive .NET names
`IsCyrillic` and `IsCJKUnifiedIdeographs` mean the inclusive blocks
U+0400–U+04FF and U+4E00–U+9FFF, respectively. Positive and negative properties
work alone or inside a character class. They are not Unicode script properties:
for example, U+0500 remains outside the Cyrillic block even though Rust's
Cyrillic script includes it. Capture groups and replacement bindings are kept.

Source evidence is the ordinal property table in dotnet/runtime v9.0.0,
commit `9d5a6a9aa463d6d10b0b0ba6d5982cc82f363dc3`,
`RegexCharClass.cs` (table entries at lines 203/212, lookup at 1828), and the
pinned Jackett's plain `new Regex` calls. The 37 exact general-category names
retain their existing behavior. Other property names and aliases reject;
block patterns containing uncertain original class nesting/set operations,
adjacent range dashes, inline options or comments reject explicitly. These
restrictions deliberately exclude some valid .NET patterns.

The source limit remains 4096 bytes; translated text is capped at 8192 bytes,
with unchanged compiled-size, DFA and nesting limits. Internal Rust-syntax
regexes bypass this definition translator. Unicode-version differences in
shorthands, UTF-16/astral matching, culture-aware case folding, final-newline
anchors and named-group numbering remain separate compatibility limits.

Exhaustive valid BMP scalar tests cover both block ranges and complements.
Synthetic responses against unchanged Anime by Belka and rutracker-ru
presets cover title stripping enabled and disabled. No presets are activated.
Corpus totals remain 583/455/423/5; thepiratebay moves from its unknown-property
failure to its existing row-template blocker. These results establish offline
semantics only, not live tracker availability.

## Bounded .NET literal regex syntax

Definition regexes now use one iterative scanner for literal syntax and the
Unicode block properties above. It reads original .NET tokens and emits Rust
syntax without rescanning generated classes. Classes containing a literal `[`,
ambiguous dash forms, or Rust set-operator spellings are parsed into .NET items
and emitted canonically. Ordinary classes stay unchanged. A `{` is literal
unless it starts an ASCII `{n}`, `{n,}` or `{n,m}` quantifier. Capture groups and
replacement bindings are preserved.

The implementation follows the pinned .NET 9 `RegexParser.cs` routines
`ScanCharClass`, `ScanCharEscape`, `IsTrueQuantifier` and `ScanBasicBackslash`.
Subtraction, POSIX-like class syntax, invalid ranges, unsupported inline options,
comments, nested quantifiers and non-BMP pattern characters reject explicitly.
The source/translation and regex-engine budgets remain unchanged. A pattern
combining block properties with other literal rewrites still rejects rather
than broadening the previously reviewed block subset.

Deprecated apostrophe-delimited named or numeric backreferences reject outside
classes, including undefined references and numeric overflow. Literal fallback
such as torrentdd's escaped-quote extraction pattern remains supported. For
potential non-ASCII reference names, the scanner conservatively rejects uncertain
cases rather than guessing .NET's Unicode category tables. These restrictions
include some otherwise valid .NET patterns; broader Unicode/UTF-16/culture and
anchor differences remain outside this implementation.

The corpus tripwire pins nine changed literal-pattern uses across girotorrent,
ilcorsaroblu, jpopsuki, puntotorrent and nekobt. The first three become offline
plans with integration blockers; the latter two expose later CSS/XML failures.
No runtime preset is added, and these synthetic/source-derived checks establish
no live tracker availability.

Parent reproduction at `12864a6` yields **583 definitions, 458 offline plans,
426 integration-blocked plans, five runtime presets and 125 first failures**.
Two independent source reviews approved the final stack, including explicit
rejection of deprecated quoted backreferences. The integrated release suite
passes 263 tests; vendor and golden bytes are unchanged.

## .NET look-around, atomic groups and backreferences

Definition regexes that need look-ahead, constant-width look-behind, atomic
groups or numbered backreferences now run on `fancy-regex` (backtracking) with
a per-search step limit and a 64 KiB input cap. All other definition regexes
stay on the linear-time `regex` crate. Balancing groups, conditionals,
variable-width look-behind and case-insensitive backreferences still reject
explicitly. 36 of the 37 definitions rejected on regex at `0deda9d` pass the
regex stage; `hawke-uno` stays rejected. Design, .NET equivalence reasoning,
bounds and validation are in
[dotnet-regex-lookaround.md](dotnet-regex-lookaround.md).


## Following HTML rows

Positive `search.rows.after` groups the snapshotted selected rows into chunks of
`after + 1`. Every child node of each following row moves into the group head,
in order, before any field extraction, removal or row filter. The emptied source
row elements stay in the document. This follows the pinned upstream
`CardigannIndexer.cs:1834-1852` and AngleSharp's `AppendNodes` behavior.
Incomplete groups fail the whole response; zero selected rows retain the existing
empty-result behavior and do not establish tracker-specific empty-page evidence.

Only plain decimal integers fitting i32 are accepted. Zero and negative values
are no-ops; positive values require HTML on every request path. Quoted numbers,
plus signs, leading zeros and other ambiguous scalar forms reject explicitly.
Individual child moves preserve every parent link, including text and comments.
Live ancestor checks reject cycles. Selected template elements and selected rows
inside inert template fragments reject rather than mis-grouping the result.
The ancestry preflight runs before any move and shares the aggregate visit budget.
Ordinary rows containing descendant templates retain existing extraction behavior.

The raw response error check and preprocessing still precede selection. The
1000-row cap applies before grouping, the 20M shared visit budget remains, and the
100k-node/depth-128 document check runs again after merging. That document check
is separately hard-bounded, as before. Budget and document failures report
`$.response`; other merge failures report `$.search.rows.after`, with fixed text.
Plans without positive `after` retain their previous execution path.

Synthetic fixtures exercise unchanged tokyotosho, indietorrents and torrentcore
definitions. This does not activate them or prove live availability. DMHY empty
results, `multiple`, `dateheaders` and other deferred semantics remain separate.

Parent reproduction at `a3c796a` yields **583 definitions, 469 offline plans,
436 integration-blocked plans, five runtime presets and 114 first failures**.
Eleven definitions gain plans; ebookbay advances to its escaped-NBSP regex
blocker. Both independent reviews cleared the final stack, and the integrated
release suite passes 275 tests. Vendor and golden bytes remain unchanged.

## Fuzzy dates: day-month-year and relative-day offsets (2026-11-01)

`filters::parse_fuzzy_date` is Rackett's bounded `DateTimeUtil.FromUnknown`. Two shapes
that pinned definitions reach were rejected and are now parsed (pinned Jackett
`fe8876d2`):

- **`28 October 2026 13:30`** (animelayer, after its month replacements; no
  `dateparse`). FromUnknown's earlier checks (`DateTimeUtil.cs:113-226`) cannot match it,
  so it goes to `FromFuzzyTime` → `DateTimeRoutines.TryParseDateOrTime`
  (`DateTimeRoutines.cs:301-333`). There the "dd month [yy]yy" regex (`:497`) reads the
  date and the "`[h]h:mm[:ss]` after the date" regex (`:359`) reads the clock.
  `day_month_year_time` accepts exactly: a 1-2 digit day, a month word, a four-digit year
  and `H:mm` or `H:mm:ss`, with whitespace normalized. The month word follows the source:
  its first three letters must be a case of the switch (`Jan` or `JAN` and so on, but only
  `Jul`), and the rest of the word must be in `[uarychilestmbro]`, ignoring case. An hour
  over 23, or a minute or second over 59, makes TryParseTime return false, so the date is
  kept at midnight, as upstream. Inputs where upstream would fall back to the time on
  *today's* date (invalid calendar day, year below 1000, a month word the switch doesn't
  know) are rejected instead. This check runs before the chrono formats, because
  `%d %b %Y %H:%M:%S` would read a `:60` second as a leap second.
- **`Today`/`Yesterday` with a ` +HH:MM` offset** (miobt's `Yesterday 22:50 +08:00`).
  `ParseTimeSpan` is `DateTime.Parse(time).TimeOfDay` (`DateTimeUtil.cs:323-326`). With an
  offset, `DateTime.Parse` converts to local time, and only the converted time of day is
  added to the day. Rackett subtracts the offset from the clock modulo 24 hours. Only
  `+HH:MM`/`-HH:MM` up to 14:00 are accepted; other offset spellings stay rejected.

**Time zone convention.** Upstream returns an unspecified-kind `DateTime` for both shapes,
which Jackett reads in the host's local time. Rackett reads it as UTC, as it already does
for `Today`/`Yesterday`, the `%Y-%m-%d %H:%M:%S` family and every `dateparse` layout
without an offset ("absent offsets assume UTC", above). A Jackett host on UTC gives the
same instants.

Seeders and leechers now ignore surrounding whitespace (for example after animelayer's
`split`), since upstream's `CoerceLong` keeps only digits, `.` and `,`
(`ParseUtil.cs:21-25`). Other non-digit characters are still rejected.

Tests: `day_month_year_time_follows_date_time_routines` and
`relative_calendar_offsets_keep_the_converted_utc_time_of_day` in
`src/cardigann_exec/filters.rs`. animelayer is admitted with its former deferred form
fixture (`tests/fixtures/form-batches/batch-oneoffs.json`).
