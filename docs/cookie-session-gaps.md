# Cookie-session gaps: test selectors, followredirect, download POST and detail pages

Eleven bundled definitions compiled everything but their supplied-cookie session. This
change models the four missing semantics the way pinned Jackett
(`fe8876d21b93dfda84eb12bd24e1c72e56fb8736`) does. Ten are admitted with cookie-harness
fixtures (`tests/fixtures/cookie-batches/batch7.json`); teamos is parked on a date-parser
gap outside this work.

Code: `CookiePlan::compile`, `search_redirect_policy`, `CookieSession::receive_request`
(`Hops`), `fetch_search`, `fetch_page` and `fetch_download` in
`src/builtin_runtime/auth.rs`; `DownloadSelectors::compile` in
`src/builtin_runtime/download.rs`.

## Login test selectors use the engine CSS

amigosshare, myspleen and noname-clubl test the login with AngleSharp `:contains`:

- `script[data-page="app"]:contains('"user":{"id":')`
- `span.key:contains("Ratio") + span.value`
- `table.menutable a.mainmenu:contains("Выход")`

The session already checks pages with the engine CSS (`query_selector_exists`), but
`CookiePlan::compile` validated the selector with `scraper`, which has no `:contains`.
It now validates with `cardigann_exec::css::Css::parse`, as `FormPlan` does. Pinned
`TestLogin` and `CheckIfLoginIsNeeded` use `QuerySelectorAll` (`CardigannIndexer.cs:935-946`,
`:984-993`).

Known limit, unchanged: a selector that is not a positive local presence check (one
with `:contains`, a combinator or `:has`) fails closed on any page that contains an HTML
`select` element (`css::check_presence_document`, docs/cookie-auth-foundation.md).
audionews, metaltracker, mvgroupforum, mvgroupmain and riperam were already admitted
under the same rule. For amigosshare, myspleen, noname-clubl and pier720 this applies to
every search page too, since `CheckIfLoginIsNeeded` checks each HTML search response. A
live search page that shows a `<select>` filter therefore fails with "Cookie
authentication document is unsupported" instead of returning results. Lifting that
guard belongs to the CSS module.

## `followredirect`

### Search paths (bjshare, teamos)

`SearchPath.Followredirect` makes upstream call `FollowIfRedirect(response)` after a
search request (`:1609-1612`, and again after a relogin at `:1627-1630`, `:1808-1811`).
With its default arguments (`BaseIndexer.cs:688-743`), each of at most five hops is
`DoFollowIfRedirect` (`:745-770`):

- a new `WebRequest` with no `Type` (GET), no headers and no Referer;
- `Cookies = overrideCookies ?? ""`, the empty string. HttpWebClient2 clears its
  container and adds no cookie for an empty string (`HttpWebClient2.cs:113-124`);
- sent with `webclient.GetResultAsync`, not `RequestWithCookiesAsync`, so the hop's
  `Set-Cookie` never reaches `UpdateCookieHeader` (`BaseIndexer.cs:598-621`).

`CookieSession` reproduces that as `Hops::Bare`. A supplied-cookie search whose paths
all declare `followredirect: true` follows up to five hops. Each hop is a GET with no
`Cookie`, no declared header and no Referer, and a hop's response cookies are not
stored. The first response's cookies are stored as before. A redirect still left after
five hops is login expiry, as `CheckIfLoginIsNeeded` treats a redirect
(`CardigannIndexer.cs:953-972`). The followed page then gets the usual login-marker
check.

The plan admits `followredirect` only as `true` or `false` on every path alike. The
session sees one search target at a time and cannot tell paths apart, so mixed paths,
the search-block flag and other values stay "Cookie auth transport semantics are not
modeled". Form sessions keep rejecting it.

Deliberate restrictions:

- A hop to another origin is rejected. Upstream would follow it (without cookies), but
  the session parses only pages from its own origin.
- Hops keep the session's bound User-Agent, as every other session hop already does
  (docs/session-search-headers.md). Upstream sends its default agent on hops.

### Root flag (pthome, ptsbao)

`Definition.Followredirect` is read in three places:

| Where | Effect |
| --- | --- |
| `TestLogin` (`:917-930`) | After a redirect to another host: rewrite the site link, save, then throw. |
| `CheckIfLoginIsNeeded` (`:955-969`) | Same, for a search redirect to another host. |
| `GetConfigurationForSetup` (`:1032-1035`) | Form logins only (`:1017-1018` returns first for others). |

For a cookie login the flag only changes the configuration after a cross-host
redirect, and that request fails either way. The session rejects every cross-origin
redirect and never moves credentials to a host a redirect names, so the same request
fails here too, without the automatic site-link rewrite. The operator changes the
source URL instead. Nothing else differs, so a root `true` or `false` is admitted.

## `download: {method: post}` (hdsky)

`Download` sets `method = POST` when `Download.Method == "post"`, an ordinal comparison
(`:2109-2110`), and passes it only to the final `base.Download(link, method, link,
headers)` (`:2189`). That is `RequestWithCookiesAndRetryAsync` with `data: null`
(`BaseIndexer.cs:537-548`). HttpWebClient2 then sends a POST without content
(`HttpWebClient2.cs:163-168`), and .NET writes `Content-Length: 0`. Redirects are
followed by `FollowIfRedirect`, whose hops are GETs.

`DownloadSelectors::compile` admits `method` only alone: `get`, or `post` on a
`login.method: cookie` definition. `CookiePlan` records it, and `fetch_download` sends
`POST` with `Content-Length: 0`, the session cookies, the declared headers and the link
as Referer. After any redirect status the hop is a GET. Other transports would download
with GET, so form, credential-POST, `login: get` and public definitions keep rejecting
`method: post`. Spellings other than `post` and `get` are rejected rather than read as
upstream's GET. `method` with `selectors`, `infohash` or `before` stays blocked: upstream
would GET-test the selected link and then POST it.

## Download requests carry the session's headers (hdturk, pier720)

`Download` sends `ParseCustomHeaders(Download.Headers ?? Search.Headers)` with
`download.before` (`:2094`, `:2106`), the selector detail page (`:2144`, `:2153`), the test
fetch and the final download (`:2162`, `:2173`, `:2188-2189`). `DownloadSelectors::compile`
rejected search headers on any login definition. It now admits them for cookie, form
and credential-POST logins, whose session sends its own rendered set. `login: get` and
API logins stay blocked.

- `download.before` (`send_before`) and the download (`fetch_download`) already sent the
  declared headers with their first request.
- The detail page now goes through `CookieSession::fetch_page`, the session form of
  `HandleRedirectableRequestAsync` (`:2014-2028`, `Hops::Headed`): the declared headers
  go with the request and with each of at most five same-origin hops, which carry the
  session cookies and no Referer. No login marker is checked, as upstream checks none
  there. The download plan's rendered headers must equal the session's own set,
  otherwise the request is rejected without naming a value. The session is tested
  first only if it is not already validated.

Previously the detail page went through `Transport::fetch`, i.e. `fetch_search`: a
login-marker check, no redirect, and a header check that a header-bearing session could
not pass. The switch is in `resolve_download_selectors` (`src/builtin_runtime.rs`). The
cookie sources already admitted with `detail` fixtures (hdarea, u2, and others)
keep passing.

hdturk renders `x-csrf-token: {{ .Config.csrf_token }}`. `builtin::cookie_session` now
calls `CookiePlan::with_settings`, so the token reaches the session (and its identity,
docs/session-search-headers.md).

## Admitted IDs (batch7)

| ID | What it needed |
| --- | --- |
| amigosshare | `:contains` test selector (Inertia JSON in `script[data-page]`, preprocessing filters) |
| myspleen | `:contains` test selector with an adjacent-sibling combinator |
| noname-clubl | `:contains` test selector; windows-1251 POST search (was `pending/noname-clubl-retry.json`) |
| bjshare | path `followredirect` |
| pthome, ptsbao | root `followredirect` |
| hdsky | `download: {method: post}` |
| hdturk | session headers on downloads, `with_settings` for `csrf_token`, POST `before`, `timeago` dates |
| pier720 | session headers (bound User-Agent) on downloads, `:has` test selector |
| torrentbd | harness only: its POST search, site-link Referer and `timeago` date already ran |

torrentbd needed no engine change. Its `timeago` date resolves against the interpreter
clock, which the layout test pins (`2026-10-29T12:00:00Z`), so the existing exact
`published` check covers it. `search_post` already matched its body.

## Not admitted

- **teamos**: path `followredirect`, the bound User-Agent and its four paged searches
  pass the full lifecycle, but its date is XenForo's
  `<time datetime="2026-10-28T10:30:00+0000">`. Pinned `DateTimeUtil.FromUnknown`
  parses that with `DateTime.Parse`. `parse_fuzzy_date` (`src/cardigann_exec/filters.rs`)
  accepts only the RFC 3339 `+00:00` form and fails the row with "unsupported fuzzy
  date syntax". The case is parked in `tests/fixtures/pending/teamos-retry.json` with
  its page. The fix is one format, `%Y-%m-%dT%H:%M:%S%z`, in `parse_fuzzy_date`. Then
  move the case into `batch7.json` and add teamos to `COOKIE_IDS` and `READY_IDS`.

## Caveats

- Supplied cookies still cannot relogin (docs/session-validation.md). Upstream resets
  `CookieHeader` to the configured cookie and retries once after a missing marker.
  torrentbd searches an AJAX endpoint (`ajgettorrents.php`). If its live fragment lacks
  the `button[href="account-logout.php"]` marker, upstream would relogin on every search,
  while Rackett fails it, as it did for aidoruonline before its fragment rule. The
  fixtures cannot settle what the live fragment contains.
- Download redirect hops still carry the session cookies. Upstream's `FollowIfRedirect`
  in `base.Download` sends none, so this is unchanged and outside this work.

## Tests

- `src/builtin_runtime/auth_sessions.rs`:
  `search_redirect_flags_admit_only_uniform_path_policies`,
  `login_test_selectors_use_the_engine_css_dialect`,
  `followed_search_redirects_are_bare_same_origin_hops`,
  `detail_pages_repeat_session_headers_on_hops_and_need_no_login_marker`,
  `download_post_sends_the_link_without_content_then_follows_with_get`.
- `src/builtin_runtime/download.rs`: `download_post_needs_a_cookie_session_and_stays_alone`,
  plus updated admission lists.
- `tests/cookie_layout_runtime.rs`: batch7 layouts and full lifecycles, with the new
  options `download_post`, `before_post`, `setting_header` and `follow_redirect`
  (docs/fixture-harness.md).
- `tests/cookie_auth.rs`: census 126 → 136 compiling cookie plans. A root
  `followredirect: true` is admitted.
