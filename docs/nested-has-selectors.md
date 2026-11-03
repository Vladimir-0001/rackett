# Nested `:has()` selectors

The CSS spec, and the `selectors` crate Rackett uses, reject `:has()` inside another
`:has()`. AngleSharp v1.8.6, which Jackett uses, accepts it. Three pinned definitions
rely on it in their row selector:

| ID | Row selector | Now blocked by |
| --- | --- | --- |
| rutor | `tr:has(td:has(a[href^="magnet:?xt="]))` | fixtures only (public) |
| selezen | `div.card:has(h4):not(:has(a[href*="/news/"])):has(a:has(span.bx-download))` | form login fixtures |
| torrentdownloads | `div.inner_container > div:has(p:has(a[href^="/torrent/"])):not(:has(span.__cf_email__))` | download resolution |

`css::rewrite_nested_has` flattens `:has(B:has(C))` to `:has(B C)` before parsing, but
only when B and C are each a single compound selector. In both forms the condition is
"a descendant matching B that itself has a descendant matching C". Because B and C have
no combinators, there is no complex-selector scoping for AngleSharp and the relative
selector to differ on, so both forms match the same elements.

Everything else is left unchanged and stays rejected:
- a complex outer or inner part (`td > b:has(a)`, `td:has(b a)`);
- a list argument;
- double nesting;
- `:has(` inside a string.

Unit tests are `nested_has_tests` in `src/cardigann_exec/css.rs`.

## Invalid literal row-selector branches (tasmanit)

tasmanit's row selector is
`table#sortabletable > tbody > tr:has(a[href]){{ if .Config.freeleech }}:has(img[src$=\"/freedownload.gif\"]){{ else }}{{ end }}`,
in a plain YAML scalar, so the backslashes stay in the selector. AngleSharp v1.8.6 reads
`\"` as an escaped identifier character, then meets `/` where it expects `]`:
`OnAttributeEnd` sets `_valid = false` (`CssSelectorConstructor.cs:326-360`).
`QuerySelectorAll` then throws `DomException(Syntax)` (`QueryExtensions.cs:454`), and the
search fails through `OnParseError` (`CardigannIndexer.cs:1828-1829, 1958-1961`). So upstream
searches work with `freeleech` off and always fail with it on.

Rackett used to require every template branch to compile, which blocked tasmanit. Now
(`Plan::compile`, `probe_row_selector`):

- A row-selector branch that is not a valid HTML selector is allowed only when the
  template substitutes no configuration values and the response is HTML or XML. Branches
  with substituted values and JSON rows selectors stay strict, as before.
- The selector rendered with the definition's default settings must still compile.
  Otherwise compilation fails with the original selector error.
- Such a plan checks the rendered selector before use. `Plan::check_settings` (so source
  creation and edits), `build_requests` and extraction refuse settings that select the
  invalid branch with `$.search.rows.selector: The configured settings select an invalid
  row selector`. The message never contains the selector or a setting value. No request
  is sent, where upstream fails after the search request.

Tests: `an_invalid_literal_branch_is_refused_only_when_settings_select_it` in
`tests/cardigann_config_selectors.rs`. tasmanit is admitted with a credential-POST
fixture (`tests/fixtures/form-batches/batch-oneoffs.json`).

## `:scope`

AngleSharp's `element.QuerySelector(sel)` binds `:scope` to `element`. Rackett
previously matched without a scope element, so `:scope` meant the document root and
torrentproject2's fields (`:scope > span > a`) never matched. Now `html_select` passes
the element a selection starts from as the scope: the row for row fields, or the
document root for `:root`-rooted selections (`Css::matches_in`). torrentproject2 is
the only pinned definition that uses `:scope`; it now needs only its public fixture.
The unit test is `scope_is_the_selection_start_element` in `selector.rs`.

## Multi-token `:contains(...)`

AngleSharp's `ContainsFunctionState` (`CssSelectorConstructor.cs:958-998`, v1.8.6) takes
every ident or string token in the argument and keeps the last one. Its tokenizer drops
comments. So `:contains(Only Upload)` matches text containing "Upload", and
`:contains("x y" z)` matches "z". Rackett used to reject any argument with more than one
token. It now follows AngleSharp, and dasunerwartete-api's
`free_button:contains(Only Upload)` compiles. Numbers and other tokens are still
rejected, as AngleSharp rejects them. A configured value substituted into a
`:contains(...)` argument (`contains_value`) must still be exactly one token, because a
multi-token value would silently match only its last word.

## Repeated compound type selectors

puntotorrent's `_bluray` field uses `a:not(:contains("HDTV"))a:not(...)`: a type
selector after a pseudo-class, with no combinator. AngleSharp adds a type selector found
anywhere in a compound to that compound (`CssSelectorConstructor.cs:211-213,465-492`),
so the repeated `a` is a no-op. The `selectors` crate rejects the repeat.
`rewrite_repeated_type` drops a type selector only when it repeats its top-level
compound's leading type, ignoring case. The other misplaced shapes stay rejected:
- no leading type (`:not(.x)a`);
- a different type (`a:not(.x)b`, which never matches);
- a namespace.

Tests are `several_argument_tokens_keep_the_last_like_anglesharp` and
`a_repeated_compound_type_is_a_no_op_like_anglesharp` in
`tests/cardigann_css_contains.rs`. Both definitions now compile:
- dasunerwartete-api needs only its public XML fixture;
- puntotorrent is admitted with a credential-POST fixture
  (`tests/fixtures/form-batches/batch24.json`). Its title is `\W`→`.` over the cell text
  plus one suffix per matching quality field. The main fixture row (`Fixture Release BluRay
  1080p 4K`) hits the BluRay, 2160p and 1080p fields. The test
  `puntotorrent_title_suffix_follows_each_quality_branch` covers the lower-case `uhd`
  BluRay alternative, Remux, WEB, HDTV, DVD/Xvid and VOSE, with expected titles derived by
  hand from the pinned selectors.
