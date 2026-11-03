# Session search headers

Cookie, form and credential-POST sessions send a definition's rendered
`search.headers`, including a literal `Cookie` header merged with the session jar,
the way pinned Jackett does. Implementation: `compile_search_headers`,
`render_headers`, `CookieSession::check_search_headers` and
`CookieSession::cookie_header` in `src/builtin_runtime/auth.rs`. The form plan uses
the same compiler (`src/builtin_runtime/form_auth.rs`).

## Upstream evidence

Pinned Jackett `fe8876d21b93dfda84eb12bd24e1c72e56fb8736`, running on .NET 9
(`Jackett.Common.csproj` targets `net9.0`):

| What | Where |
| --- | --- |
| `ParseCustomHeaders` renders the first value of each header with the request's template variables. | `CardigannIndexer.cs:2192-2208` |
| Login, the form landing page and TestLogin send `Login.Headers ?? Search.Headers`. | `:572`, `:903-904`, `:1021` |
| Searches send `Search.Headers`, with no Referer argument. | `:1605-1607` |
| Downloads send `Download.Headers ?? Search.Headers`: before, selector pages and the final download. | `:2094`, `:2116`, `:2144`, `:2188` |
| `RequestWithCookiesAsync` passes the stored `CookieHeader` as `WebRequest.Cookies` and the custom headers as `WebRequest.Headers`. | `BaseIndexer.cs:598-621` |
| `BaseIndexer.Download` passes the link itself as Referer. | `BaseIndexer.cs:537-548` |
| Redirect hops (`FollowIfRedirect`) are new requests without custom headers. | `BaseIndexer.cs:688-718`, `:745-770` |
| HttpWebClient2 loads `WebRequest.Cookies` into its CookieContainer, adds every custom header except `Content-Type`, keeps a custom User-Agent, then sets `Headers.Referrer` from the Referer argument. That setter replaces a custom Referer. | `HttpWebClient2.cs:107-148` |

.NET 9 `SocketsHttpHandler` (`dotnet/runtime` `release/9.0`,
`src/libraries/System.Net.Http/src/System/Net/Http/SocketsHttpHandler/HttpConnection.cs`)
merges the two cookie sources when it writes an HTTP/1.1 request:

- `:363-372`: the container's cookie header for the URL, or none.
- `:443-448`: when the request has a `Cookie` header, its value is written first,
  then `"; "`, then the container's cookies.
- `:465-470`: without a `Cookie` header, the container's cookies are written alone.

Jackett's `HttpRequestMessage` uses the default HTTP/1.1, so it sends one
`Cookie: <definition value>; <session cookies>` header. Response cookies update only
the stored session. The definition's literal never enters the jar.

## Semantics

Headers are compiled once and rendered when the session is created. The request
kinds below match the pinned calls:

| Request | Declared headers | Referer |
| --- | --- | --- |
| Login test (cookie and form sessions) | first request | declared |
| Search page | yes | declared |
| Form landing page | yes | site link (replaces declared) |
| Form or credential POST | first request | login page or site link (replaces declared) |
| `download.before` | first request | the release link (replaces declared) |
| Download selector detail page (`fetch_page`) | every request and hop | none |
| Download | first request | the download link itself, for every session |
| Followed search redirect hop (`followredirect`) | no | none, and no cookie |
| Any other redirect hop | no | unchanged from before this change |

The `Cookie` header is the definition's literal, `"; "`, then the jar. Without a jar
cookie it is sent alone. Redirect hops carry only the jar, except followed search hops,
which carry no cookie ([cookie-session gaps](cookie-session-gaps.md#followredirect)).

The request plan renders search headers independently. Each session search
compares the plan's rendered set with the session's own (header names
case-insensitive, values exact) and rejects any difference. The session then sends
its own set.

### Admitted declarations

- At most 16 headers, one value each. Upstream ignores later values, so more
  than one is rejected rather than dropped. Header names are unique
  case-insensitively and must be valid HTTP tokens.
- Rejected names: `Host`, `Connection`, `Content-Length`, `Content-Type`, `Expect`,
  `Keep-Alive`, `Proxy-*`, `TE`, `Trailer`, `Transfer-Encoding` and `Upgrade`.
- `User-Agent` must be `{{ .Config.useragent }}` with one `useragent` text setting.
  It stays bound to the session client (docs/cookie-auth-foundation.md).
  Form sessions bind no User-Agent, so they reject one.
- Values are literal visible ASCII and `{{ .Config.sitelink }}` or `{{ .Config.<text
  setting> }}` references. `cookie` and `useragent` are not allowed in other headers.
  No other template syntax is admitted.
- `Cookie` must be literal RFC 6265 cookie-pairs.
- `Referer` must start with `{{ .Config.sitelink }}`, followed by literal text.
- A referenced setting must be non-empty visible ASCII of at most 4096 bytes. The
  error names the setting (public schema), never its value. Rendered values are
  limited to 8192 bytes.

### Secrets and session identity

Header values (for example a CSRF token) are never put in errors, logs, cache keys
or download links. Download tickets stay opaque.

When a source declares headers, the rendered set is digested into the session
identity after the seed and User-Agent. A changed token, site link or literal then
starts again from the configured seed. That matches pinned `ApplyConfiguration`,
which resets `CookieHeader` from the configuration. Sources without declared
headers keep their existing identity and stored sessions.

## Admitted IDs

Fixtures: `tests/fixtures/cookie-batches/batch6.json`, with the harness options
`referer` and `literal_cookie` (docs/fixture-harness.md).

- gaytorrentru: `Referer: {{ .Config.sitelink }}search.php?view=legacy` plus the
  bound User-Agent.
- hdgalaktik: `Cookie: torrentviewblock=1`. Its `rows.dateheaders` already ran
  (docs/date-headers.md); only the header blocked its cookie plan.
- rustorka: `Cookie: godbayadblock=godbayadblock` (windows-1251).
- tormac: `Cookie: tr_simple=0` (windows-1251).

## Not admitted

- hdturk (`x-csrf-token: {{ .Config.csrf_token }}`), pier720 (User-Agent) and
  torrentbd (`Referer` plus User-Agent) were admitted later with batch7: download
  requests now carry the session's headers, the detail page goes through
  `CookieSession::fetch_page`, and `builtin::cookie_session` calls
  `CookiePlan::with_settings`. See [cookie-session gaps](cookie-session-gaps.md).
- Form sources with search headers (infire `Referer`, tapochek literal `Cookie`,
  eztvl literal `cookie`): `FormPlan` now accepts the headers and unit tests cover
  them (`form_sessions_send_declared_headers_with_login_and_search_requests`).
  Admission needs form-harness fixtures, and eztvl and kinozal have other
  blockers of their own.
- Redirect hops still carry the session's bound User-Agent, while pinned
  `FollowIfRedirect` sends Jackett's default. This is unchanged.

## Current state (2026-11-01)

gaytorrentru, hdgalaktik, rustorka and tormac are admitted with cookie fixtures. All
cookie, form, builtin and NexusPHP test targets pass, and clippy and fmt are clean.
