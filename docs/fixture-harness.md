# Fixture harness options

Bundled Cardigann definitions are admitted only after a source-pinned fixture case passes
one of three harnesses. Each case is a JSON object in a manifest the harness includes;
every manifest uses the same `Case` struct, documented in the test file itself.

| Harness | Test file | Manifests | Fixture pages | Allowlist |
| --- | --- | --- | --- | --- |
| Public (no login, `login: get` API keys) | `tests/public_layout_runtime.rs` | `public-layouts/cases.json`, `public-batches/batch*.json` | `public-layouts/<id>.html` or `.json` | `READY_IDS`, `GET_IDS` |
| Form login and credential POST | `tests/form_layout_runtime.rs` | `form-layouts/cases.json`, `post-layouts/cases.json`, `form-batches/batch*.json` | `form-layouts/`, `post-layouts/` or `cookie-layouts/` (by `search_fixture`) | `FORM_IDS`, `POST_IDS` |
| Supplied cookie | `tests/cookie_layout_runtime.rs` | `cookie-layouts/cases.json`, `cookie-batches/batch*.json` | `cookie-layouts/<id>.html` | `COOKIE_IDS` |

A new manifest must also be added to the census lists in `tests/builtin_runtime.rs` and
`tests/nexusphp_cookie_runtime.rs`. Those tests check that the ready catalog equals the
union of all manifests.

Every case runs an offline extraction test (fields, promotions, empty page, broken
size or missing title) and a full engine lifecycle on a loopback fixture server. The
lifecycle covers search, cache, download, edit, restart, error and challenge pages, and
recovery. Set `RACKETT_PUBLIC_CASES`, `RACKETT_FORM_CASES` or `RACKETT_COOKIE_CASES`
to a comma-separated ID list to run only those cases while iterating. Tests that look
up a specific ID then fail, so run the full target before submitting.

Public HTML/XML sources also need an `empty_html_container` entry in `src/builtin.rs`.
That is the element an empty result page must still contain. Without it, an error page
with no rows would count as "no results", and lifecycle mode 2 fails.

Every option below applies to one case only. A case that leaves an option out keeps
the default assertion.

## Public harness (`tests/public_layout_runtime.rs`)

| Option | Meaning |
| --- | --- |
| `id`, `sha256` | Definition ID and SHA-256 of its pinned source; a changed source fails. |
| `category` | Query `cat` and an expected release category (may be a custom `100000+id` category to keep `{{ range .Categories }}` inputs narrow). |
| `request_count` | Requests one keyword search builds (pages). |
| `keywordless_request_count` | Requests for a keywordless query when that differs. |
| `search_path` / `search_paths` | Exact request path, or the set of allowed page paths. |
| `query_pair` | `[key, value]` the request query must contain (`["", ""]` when the keyword is in the path). |
| `search_post` | The search is `method: post` (noname-club): requests must be POST with a form body, and `query_pair` is matched in the body, decoded with the definition charset. Other cases must send GET without a body. |
| `download_path`, `details_path` | Expected link suffixes; `download_path: "magnet"` expects an infohash/magnet release. |
| `detail` | `download.selectors`/`infohash` source: `html` is the detail page served at `download_path`, `resolved` the torrent path or exact magnet it must yield. With `before` (a request path and query), `html` is served there instead and the release link must not be fetched (`infohash.usebeforeresponse`, magnetdownload). |
| `seeders`, `peers`, `published`, `title` | Expected field values (`date: now` yields `2026-10-29T12:00:00Z`). |
| `expected_size` | Expected size in bytes (default 1 GiB). |
| `optional_size` | The size field is optional: broken size text yields no size, not an error. |
| `fixed_size` | The size is a literal (`text: 512 MB`); broken text still yields `expected_size`. |
| `defaulted_size` | The size has a `default`: broken text yields this size (byrutor's `0 B` → 0). |
| `malformed_size_dropped` | The row selector drops a row whose size cell contains `-` (torrentsome's `:not(:has(div:nth-child(3):contains("-")))`), so the broken-size page (`bad-size`) must give no rows rather than an error. |
| `download_factor` | Expected `downloadvolumefactor` (default `"0"`). |
| `delay_ms` | Expected pinned `requestDelay`; the cold search must pay every inter-page delay. |
| `timeout_seconds` | Request timeout the source needs. Sets the engine request timeout and a lifecycle budget of 5× (at least 180 s). Used for torrentcore's 10.1 s delay. |
| `json`, `empty_body` | JSON fixture, and the source's no-results body when it cannot use rows markers. |
| `container` | Expected `empty_html_container` from `src/builtin.rs` (or `null`). |
| `settings` | Source settings, e.g. an API key; leak-checked in presented results. |
| `probe` | Exact `login: get` probe request, answered with empty caps and not counted. |
| `expected_headers` | Rendered headers every request must carry (lower-case names). `$SITELINK` in a value is the fixture site link, e.g. `{"referer": "$SITELINK"}` for `Referer: {{ .Config.sitelink }}`. |

The broken-size corruption replaces `1 GB`, `1 ГБ`, `1\u{a0}GB`, `1&nbsp;GB` and
`1073741824`. The non-breaking-space forms cover definitions that select sizes with
`:contains(\00a0GB)`, such as rutor.

## Form and credential-POST harness (`tests/form_layout_runtime.rs`)

| Option | Meaning |
| --- | --- |
| `id`, `sha256` | Definition ID and pinned-source hash. |
| `search_fixture` | `form:<id>`, `post:<id>` or a cookie-layout name; the page file. |
| `direct_post` | Credential POST (`login: post`, `POST_IDS`) instead of a landing form. |
| `form_path`, `submit_path`, `form_id`, `form_attributes` | Landing page path, form action, form ID and optional raw form attributes. |
| `landing_redirect` | The declared login path answers 302 to `form_path` (definition `followredirect`, lastfiles). The first landing hit is this path, relogins count one more request, and the credential POST must resolve the form action against it and carry it as Referer (`CardigannIndexer.cs:724`, `:804`). |
| `landing_html`, `landing_cookie` | Extra landing markup and a required `login.cookies` seed. The markup is appended after the form and holds tokens for `selectorinputs`, or the elements that `login.selectors: true` input keys match. `expected_inputs` then uses the matched elements' `name`s (speedmasterhd: `fixture_u7`). Leave out an `optional` selector input's element to test that it is skipped (pornolab `cap_sid`, tapochek `confirm_id`). |
| `expected_inputs` | Exact credential body; `{{ .Config.x }}` values are resolved from the case. |
| `configured_settings` | Extra secret settings (passkeys, PINs). The catalog must mark them secret, and they are leak-checked. |
| `public_settings` | Declared non-secret settings (e.g. a `category-id` select in the search path). The catalog must mark them non-secret. They are also used in the offline context and are not leak-checked. |
| `error_html`, `search_error_html` | Login failure page and an optional `search.error` page. |
| `no_test` | The definition has no `login.test`: no validation request, and only a redirect means expiry. |
| `login_path` | TestLogin path (or the path that must never be requested with `no_test`). |
| `search_path`, `keyword`, `keyword_value` | Search path and a query pair that identifies a search request. When the keyword is in the path (racingforme, wihd), the path carries it and `keyword` names a fixed input. A definition with no search inputs at all (torrentleech) sets `keyword: ""`: the exact `search_path`, which contains `keyword_value`, then identifies a search. |
| `category`, `custom_category`, `category_input` | Query category, an extra expected release category and a required category pair. |
| `search_post` | The search is `method: post`; pairs are matched in the body. A non-UTF-8 body must use `HttpUtility`'s lower-case escapes and is decoded with the definition charset, keeping repeated keys, before matching (file-tracker). |
| `json`, `empty_body` | JSON search response and its empty body. |
| `detail` | Detail-page download resolution, as in the public harness. |
| `before_path`, `before_fields` | `download.before` request and its required POST fields. |
| `request_count` | Search pages per query. |
| `download_path`, `details_path` | Expected link suffixes. If `details_path` is absent, the definition declares no `details` field and releases must have none (acrossthetasman). |
| `published` | Expected date. |
| `expected_title`, `expected_size`, `expected_seeders`, `expected_peers` | Overrides for the defaults: `Fixture Release`, 1 GiB, 11 seeders and 15 peers. |
| `missing_title` | Title the definition yields when the title text is removed (e.g. leftover `Uploaded by …` text) instead of an error. |
| `free_markup`, `normal_markup`, `free_download`, `free_upload`, `normal_download`, `normal_upload` | The `@@PROMO@@` replacements and the expected factors for each. |
| `timeout_seconds` | Request timeout the source needs; lifecycle budget 5× (at least 300 s). |
| `user_setting` | The login identity setting is not called `username` (magnetz: `email`). The catalog must mark it secret and `{{ .Config.<name> }}` in `expected_inputs` resolves to the fixture user. |
| `password_setting` | The login secret setting is not called `password` (Bittorrentfiles: `pass`). The credentials, the secret check, the password-change step and the `{{ .Config.<name> }}` value in `expected_inputs` use it. |
| `no_credentials` | The definition declares no settings and logs in with literal inputs only (comicat and miobt: `visitor_test: human`). No username/password settings, and the password-change step of the lifecycle is skipped (one login, then one relogin after expiry). Combine with `no_test`. |
| `literal_cookie` | The definition declares this literal `Cookie` search header (eztvl). Pinned Jackett sends declared headers on every request, so each request, including the credential POST, must carry it alone or followed by `"; "` and the session cookie. The fixture strips it before its own cookie checks, so a stale-cookie check on the credential POST still runs against the jar. |
| `download_path: "magnet"` | The release link is itself a magnet (magnetz `download`, eztvl `a.magnet`) or is built from an `infohash` field (comicat, miobt). The layout check accepts a `magnet:?` link containing the fixture hash or an `infohash` attribute equal to it. The lifecycle sends no download request: the presented link is the magnet unchanged, and the ticket, edit and foreign-link checks that need a download are skipped (the foreign-origin check still runs). |

`before_path` normally names a path only the `download.before` request uses. tmghub posts
its thank-you comment to the release's own details URL (`{{ .DownloadUri.AbsoluteUri }}`),
which shares the `/index.php` path with searches, the login test and the login submit. When
`before_path` equals `search_path`, only a POST that is not the login submit counts as the
`before` request; GETs go to the normal search, test and detail handling.

`form-layouts/deferred.json` (hdtorrents) holds a case that is deliberately not
admitted. It stays out of `form-batches/`, which `tools/admit_form_batches.py` admits
wholesale. Its test reproduces the blocker exactly: a date the bounded fuzzy date parser
rejects. animelayer, formerly in `deferred-animelayer.json`, moved to
`form-batches/batch-oneoffs.json` on 2026-11-01.

### Rows that match their own field selectors

Like upstream's `handleSelector` (`Dom.Matches(selector) ? Dom : QuerySelector(Dom, selector)`,
`CardigannIndexer.cs:1305`), a field selector is tried on the row element itself before
its descendants. torrentsome's rows are sibling `div.topic-item` elements and its fields
use `div:last-child` (date) and `div:nth-last-child(2)` (size). On the site, and upstream,
the last two rows of a page therefore read the whole row text for those fields and fail.
The torrentsome fixture puts two non-row elements after its single row, so it tests the
ordinary rows. This is upstream behaviour, not a harness gap.

## Supplied-cookie harness (`tests/cookie_layout_runtime.rs`)

| Option | Meaning |
| --- | --- |
| `id`, `sha256` | Definition ID and pinned-source hash. |
| `login_path`, `no_test` | TestLogin path; or no `login.test` declared. |
| `search_path` / `search_paths`, `keyword`, `keyword_value`, `category_input` | Request shape, as in the form harness. |
| `request_count` | Search pages per query. |
| `category`, `custom_category` | Query category and an extra expected category. |
| `user_agent` | The definition renders `search.headers.User-Agent` from a `useragent` setting. The lifecycle checks the header and a changed agent. Such cases also need an arm in `browser_cookie_layouts_keep_provider_specific_fields_and_alternative_branches` that tests their own settings or branches. |
| `search_post` | As in the form harness: the search is POST and its decoded body carries the keyword and category pairs. |
| `referer` | The definition declares `Referer: {{ .Config.sitelink }}<suffix>`; login tests and searches must carry it ([session search headers](session-search-headers.md)). Every case checks that downloads send their own link as Referer and that other requests send no Referer unless one is declared. |
| `literal_cookie` | The definition declares this literal `Cookie` search header. Every request must send it first, then `"; "` and the session cookie. |
| `setting_header` | `{"header", "setting", "value"}`: a declared search header rendered from a text setting (hdturk's `x-csrf-token` from `csrf_token`). The lifecycle configures the setting, and every request except a followed search hop must carry the header with that value. |
| `download_post` | `download: {method: post}` (hdsky): the release link must arrive as a POST with `Content-Length: 0`, no `Content-Type` and no body, with its own link as Referer. |
| `before_post` | The path of a `download.before` POST without inputs (hdturk's `/torrents/42/thanks`). It must carry an empty `application/x-www-form-urlencoded` form, the release link as Referer, and come before the detail page. |
| `follow_redirect` | Every search path declares `followredirect: true` (bjshare). After the other modes, mode 6 redirects each search once to `/fixture-moved`. The session must follow with a GET carrying no cookie, no Referer and no declared header, and the search must return its row ([cookie-session gaps](cookie-session-gaps.md#followredirect)). |
| `json`, `empty_body`, `detail` | As in the form harness. A cookie `detail` page needs no login marker: sessions fetch it without the login check, as upstream's `HandleRedirectableRequestAsync` does. |
| `download_path`, `details_path`, `published`, `expected_title`, `missing_title` | Expected fields; the size is always 1 GiB, with 11 seeders and 15 peers. |
| `free_markup`, `normal_markup`, `free_download`, `free_upload`, `normal_download`, `normal_upload` | Promotion markup and factors (normal factors default to 1). |

## Harness additions on 2026-11-01

- Cookie harness: `setting_header`, `download_post`, `before_post` and
  `follow_redirect` for `cookie-batches/batch7.json`
  ([cookie-session gaps](cookie-session-gaps.md)). `browser_cookie_queries_*` now folds
  a POST search body into the matched URL (torrentbd). Relative `timeago` dates need
  no option: the layout test pins the interpreter clock to `2026-10-29T12:00:00Z`, so
  `published` is exact (torrentbd `2h`, hdturk `2 sa`).

- `search_post` in the public and cookie harnesses, and charset-aware body decoding
  in all three, for non-UTF-8 POST searches (noname-club, file-tracker;
  [legacy-encodings.md](legacy-encodings.md#search-post-bodies)).
- `keyword: ""` in the form harness for torrentleech, whose keyword is only in the path.
- The fixture servers of all three harnesses answer 404, without recording a hit, to
  requests whose `Host` is not `127.0.0.1:<port>`. On a shared machine an unrelated local process
  (`user-agent: node`, `Host: localhost:<port>`) probed the listening port, which
  made the hit-count assertions flaky. The engine always uses the `127.0.0.1` base.

## Harness gaps closed on 2026-10-31

The batch agents reported these as harness limitations:

| ID | Gap | Resolution |
| --- | --- | --- |
| acrossthetasman | No `details` field | `details_path` is optional; admitted (credential POST) |
| byrutor | Size defaults to `0 B`; 305 categories all map to PC/Games | `defaulted_size`; custom category 100001; admitted |
| rutor | Size text uses a non-breaking space | Corruption covers `1\u{a0}GB`/`1&nbsp;GB`; admitted |
| torrentcore | 10.1 s `requestDelay` exceeds the 180 s budget under load | Public `timeout_seconds`; admitted |
| ebookbay | `$.certificates` blocker in the offline test | The offline test already allowed it; admitted with `optional_size` |
| animelayer | Non-secret `category-id` setting | `public_settings` added. The lifecycle's password change now resends them, since a patch keeps only secret settings (`builtin::preserve_secrets`). Admitted on 2026-11-01 once `28 October 2026 13:30` dates parsed ([cardigann.md](cardigann.md#fuzzy-dates-day-month-year-and-relative-day-offsets-2026-11-01)) |
| ex-torrenty | `unreachable!()` provider arm | Arm for its multilang title and ordered category case; admitted |
| wihd | Keyword in the search path | Expressed as racingforme does (the path carries the keyword; `freeleech=0` marks searches); admitted |

Known runtime limitation: byrutor maps all 305 site categories to PC/Games, so a
`cat=4000` query expands to more than the 256-input request limit and fails. Narrow
custom categories and uncategorised queries work.
