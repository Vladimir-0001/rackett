# JSON row shapes: `multiple`, parent fields and missing attributes

Three `rows` properties used only by Cardigann's JSON branch now run as in pinned
Jackett (`CardigannIndexer.cs:1681-1736`, commit `fe8876d2`). HTML and XML responses
ignore `multiple` and `missingAttributeEqualsNoResults`, as upstream does.

- **`multiple: true`** with `attribute`. Each selected row's attribute must be an array
  of objects, and every object becomes its own release (`selObj.Values<JObject>()`). A
  movie with three torrents gives three releases. A non-array attribute, or an array
  containing non-objects, fails the response.
- **Parent fields (`..name`).** A field whose selector starts with `..` reads the
  original row instead of the attribute object. `handleJsonSelector` trims the leading
  dots, so this is a plain lookup on the parent, not a JSONPath deep scan.
- **`missingAttributeEqualsNoResults: true`.** A rows selector that resolves to nothing
  or to a non-array means no results instead of an error. A row whose `attribute` is
  missing is skipped. Without the flag, both remain errors.
- Both flags must be YAML `true` or `false`.

Implementation: `selector::json_row_pairs` returns (parent, release object) pairs, and
`Selector::json_parent` picks which one a field reads. Upstream collapses *any* failure
inside its rows helper into "no rows". Rackett does that only for a missing or non-array
rows token. Errors from row filters or limits still fail the response.

## Effect on the bundled definitions

| ID | Uses | Now blocked by |
| --- | --- | --- |
| yts | `multiple`, `..` fields, `missingAttributeEqualsNoResults` | fixtures only (public) |
| hebits | `multiple`, `..` fields | cookie fixtures (`CookiePlan` compiles) |
| sportscora | a `..id` field | API-key fixtures (`GetLogin` compiles) |
| hdforever-api | `missingAttributeEqualsNoResults` | an unsupported `login: get` property |

Tests: `tests/cardigann_json_rows.rs`.
