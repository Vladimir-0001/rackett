# `login: get` paths, login headers and API hosts

The five definitions whose `login: get` block the probe in
[session-search-shapes.md](session-search-shapes.md#login-get-api-key-probes) did not
model, and what was done about each. Upstream is Jackett `fe8876d2`,
`src/Jackett.Common/Indexers/Definitions/CardigannIndexer.cs`.

## Upstream behaviour

- `DoLogin` renders the login headers once: `ParseCustomHeaders(Login.Headers ??
  Search.Headers, variables)` (`:572`).
- The `get` branch (`:814-830`) renders each input with `applyGoTemplateText` and adds it
  to a `NameValueCollection`. The URL is `resolvePath(applyGoTemplateText(Login.Path,
  variables) + "?" + queryCollection.GetQueryString())`. The path template has no
  modifier, so a substituted setting goes into the URL raw, not URL-encoded (search paths
  use `WebUtility.UrlEncode`, `:1543`). The `?` is appended even when the path already
  has a query and there are no inputs.
- It clears the cookie header, sends one GET with `referer: SiteLink` and the headers,
  keeps the response cookies, and runs `checkForError` (`:538-562`): HTTP 401, or any
  `error` selector matching the body parsed as HTML, fails.
- `TestLogin` (`:892`) returns early without `login.test`.
- Definition-level `followredirect` is read at `:923` (inside `TestLogin`), `:961`
  (`CheckIfLoginIsNeeded`) and `:1032` (form landing page). In `CheckIfLoginIsNeeded`
  (`:951-969`), a search redirected to another domain makes upstream overwrite the
  stored site link with that domain and then throw. The request fails either way.

## What Rackett now admits (`src/builtin_runtime/get_login.rs`)

1. **Templated settings in the path's query.** After the path's first `?`, a path may
   contain `{{ .Config.name }}` for a declared `text` or `password` setting. Before the
   `?` no template is allowed. Other template syntax fails to compile. The value is
   inserted raw, as upstream does. It must be 1 to 1024 URL unreserved characters
   (`A-Z a-z 0-9 - . _ ~`). For these characters the raw and the encoded renderings are
   the same, and the value cannot add a parameter, a fragment or a path. Any other value
   fails with an error that names the setting but never shows its value. Upstream would
   send such a value unencoded, so the request would change shape; Rackett refuses
   rather than guess. Setting values in the path count as the probe's credential.
2. **The appended `?` is kept.** SceneTime's probe is
   `rss.php?apikey=KEY&view=html?`, exactly as upstream builds it.
3. **`login.headers`** is admitted only when it equals `search.headers` key by key and
   value by value. The probe already carries the plan's rendered search headers, so it
   then sends exactly `Login.Headers ?? Search.Headers`. Different login headers still
   fail to compile.
4. **Definition-level `followredirect: true|false`** is admitted for `get` logins. A
   `get` login without a `test` reaches none of the three uses above that could change
   a request. Rackett's transport rejects a redirect to another origin, so the request
   fails as upstream's does. Rackett never rewrites the configured URL. Search-level
   `followredirect` still fails to compile, as before.
5. **Absolute paths** (`http://`, `https://`, `//`) fail to compile with "GET login on
   another host needs a credentialed API origin" (see below).

Paths still reject `#`, control characters and whitespace outside a template. The limits
are unchanged: 2048 bytes per path and 32 inputs.

## Definitions

| ID | Upstream login | Result |
| --- | --- | --- |
| scenetime-api | `rss.php?apikey={{ .Config.apikey }}&view=html` | admitted (`GET_IDS`, `READY_IDS`, container `channel`) |
| hdforever-api | `api.php?endpoint=self`, `login.headers` equal to the search `X-Api-Key`, root `followredirect: true` | admitted (`GET_IDS`, `READY_IDS`) |
| abnormal-api | `https://{{ .Config.apiurl }}/api/Release/Search?name=test&resultByPage=1` | blocked: credentialed API origin |
| v3x | `https://{{ .Config.apiurl }}/indexer/search` with `apikey` input | blocked: credentialed API origin |
| yggreborn-api | `https://{{ .Config.apiurl }}/api` with `apikey` input | blocked: credentialed API origin |

### Why the three API-host definitions stay blocked

All three log in and search on `https://{{ .Config.apiurl }}` (defaults `api.abn.lol`,
`api.v3x.club`, `api.yggreborn.org`) and send the key there: abnormal-api as
`Authorization: Bearer`, the other two as an `apikey` query value. Running them would
need two things that [api-origins.md](api-origins.md) leaves out on purpose:

- **Credentials on the API origin.** Rule 4 allows API origins for public sources only.
  `Transport::fetch_api` refuses a source with a `login: get` probe. Lifting that rule is a
  policy decision for the owner of `src/builtin.rs` and `src/builtin_runtime.rs`: the
  probe and the searches would go cookie-free to the configured API host, as upstream
  sends them.
- **Where downloads are.** Their `download` comes from the API response (abnormal-api
  `downloadUrl`, v3x `download`, yggreborn-api `enclosure@url`). abnormal-api's own
  comment (`abnormal-api.yml:141`) says the API returns `https://api.abn.lol/...` URLs.
  No public document shows the other two. The YggReborn guide is behind a login. If the
  links are on the API host, rule 6 fails every search ("Release download link must
  remain on the site origin"). For abnormal-api the download would also need the Bearer
  key on the API host. That is the polishtracker-api case: a ticketed, credentialed
  download origin.

A fixture that put their links on the site would invent behaviour, so they are not
admitted. With credentialed API origins approved, the compile step needs only the
absolute-path branch and the probe routed through `fetch_api`. A capture of each API's
response would show where the download links are.

## Fixtures

`tests/fixtures/public-batches/batch18.json` with
`tests/fixtures/public-layouts/scenetime-api.html` (Torznab RSS) and
`tests/fixtures/public-layouts/hdforever-api.json` (Gazelle JSON `response.results`).
Neither needs a new harness option:

- scenetime-api: `"probe": "/rss.php?apikey=fixture-source-key&view=html?"`. The
  trailing `?` is part of the exact request URI.
- hdforever-api: `"probe": "/api.php?endpoint=self"`, `"expected_headers":
  {"x-api-key": "fixture-source-key"}` on the probe, the searches and the download,
  `delay_ms` 10000 (`requestDelay: 10`) with `timeout_seconds` 60. `empty_body` is an
  empty `results` array. `error_body` is HTML, because with
  `missingAttributeEqualsNoResults` the default JSON error body would read as no
  results.

## Validation (2026-11-01, branch head `686cdaf` plus the `src/builtin.rs` hand-off)

The admission lines for `src/builtin.rs` (`READY_IDS`, `GET_IDS`, the scenetime-api
`channel` container) belong to the integrator, so they are handed over as a patch. On
this branch alone the two batch18 cases fail as not ready.

- `cargo test --lib`: 196 passed, 1 ignored. The get_login unit tests cover templated
  paths (raw unreserved values, the trailing `?`, rejected values with no value in the
  error, templates before `?`, non-setting and malformed templates, absolute paths),
  login headers equal and unequal to the search headers, and root and search-level
  `followredirect`.
- `cargo test --test public_layout_runtime` (full): 7 passed, 1 ignored (163 s),
  including both new lifecycles.
- `cargo test --test builtin_runtime`: 10 passed. `cargo test --test
  nexusphp_cookie_runtime`: 5 passed. Both include `batch18.json`.
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` are clean.
