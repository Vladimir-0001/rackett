# Session validation: once per session, not per search page

Cookie, form and credential-POST sessions (`CookieSession`, `src/builtin_runtime/auth.rs`)
used to re-request the login test page before every search page. Pinned Jackett
(`CardigannIndexer.cs`, commit `fe8876d2`) does not; this change matches it.

## Upstream evidence

- `ApplyConfiguration` (`:1102-1112`) runs `DoLogin` then `TestLogin` once (`:1106-1107`).
- `PerformQuery` (`:1438`) loops over the search paths (`:1517`) and sends each search
  request (`:1604-1607`) with no `TestLogin` before it.
- After the response, JSON paths (`:1616-1632`) and HTML paths (`:1798-1813`) call
  `CheckIfLoginIsNeeded` (`:951-997`): a redirect, or an HTML page missing the test
  selector, means expiry. Then `Relogin` (`:874-890`: `DoLogin`, then `TestLogin`) runs
  and the same request is sent once more. There is no second relogin for that path.
  XML responses are not checked.
- `Download` (`:2083`) neither tests nor relogs.

## Semantics in Rackett

`SessionState` carries an in-memory `validated` flag. It lives in the state that the
`SessionRepository` shares by identity (source, definition, origin, credentials or
seed cookie, User-Agent), so:

- A full validation sets it: login plus TestLogin, or the test of a stored or supplied
  session. Without `login.test` validation sends nothing and still sets it.
- `fetch_search` tests only an unvalidated session. Each search clears the flag before
  its request, and only an accepted page sets it again. An expiry signal, an HTTP or
  transport error, a challenge or a cancelled deadline therefore leaves the session
  unvalidated, and the next search tests it first.
- Expiry on a search page (redirect, missing marker, or 401 with a test selector) relogs
  once and retries that page once, as before. A failed retry returns the error.
- Supplied cookies cannot relog. Their expired search fails, and the next search fails
  visibly at its test request.
- A runtime replacement with the same identity (a rename, a timeout change) keeps the
  same cookies and the same flag. A credential, seed cookie, origin or User-Agent change
  is a new identity, so the new state starts unvalidated.
- The flag is never persisted, so the first use after a restart validates once.
- The API source test (`POST /api/indexers/{id}/test`) runs a search, so it follows
  these search rules: a validated session sends only the search request.

Deliberate differences from upstream:

- **First use after a restart validates once.** Upstream trusts cookies restored from
  its config until a search fails. Rackett's durable session may be days old, and the
  test page is the strongest signal it has: typed JSON searches, aidoru's fragment
  search and test-less definitions cannot prove a login from the search response.
  One extra request per restart keeps "Login failed" errors at the test page, where the
  supplied-cookie error is precise, and costs nothing on later searches.
- **Downloads, `download.before` and the explicit `test()` still validate every time.**
  Upstream downloads never relogin, so an expired session would fail every download
  until a search noticed. Keeping the check preserves Rackett's existing recovery for
  single, user-initiated requests.

## Request counts

gay-torrents: credential POST, `login.test`, five search pages, `requestDelay: 2`.
S = credential POST, T = TestLogin, s = search page.

| Step | Before | After |
| --- | --- | --- |
| First search, no session | S T s (T s)x4 = 11 | S T s x5 = 7 |
| Next search, valid session | (T s)x5 = 10 | s x5 = 5 |
| First search after restart | (T s)x5 = 10 | T s x5 = 6 |
| Page 1 expires mid-session | T s S T s (T s)x4 = 13 | s S T s x5 = 8 |

At 2 s per gap, recovering an expired session drops from at least 24 s to 14 s of
pinned delay. The `timeout_seconds: 120` overrides on gay-torrents, jpopsuki and
newstudiol were removed, and pttime is back to its earlier 40. `lifecycle_budget` stays
for sources that do need a longer deadline.

## Tests

- Unit tests (`src/builtin_runtime/auth_sessions.rs`): no test request on later pages of
  a validated form session, an expired page relogs exactly once and retries, a failed
  retry leaves the session unvalidated, restart and same-identity replacement validate
  once, and supplied cookies validate once and fail visibly on expiry.
- `tests/cookie_layout_runtime.rs`: first search is one preflight plus one request per
  page; restart and changed User-Agent validate once; mode 1 sends no preflight;
  modes 3 and 4 run only their failing preflight after mode 2's expired page.
- `tests/form_auth.rs`: the second search of a validated form session sends no test.
  `tests/musopia_runtime.rs`: the API test search after a validated search sends one
  request. `tests/cookie_auth.rs` needs the same kind of update (aidoru mode 1 sends
  no preflight; the User-Agent test's redirect search has none); see Current state.
- `tests/form_layout_runtime.rs`: no request between search pages after TestLogin;
  restart is one TestLogin plus pages; a changed password is one login plus pages;
  mode 6 (page 1 expires) is the expired page, one relogin and all pages; modes 3, 4
  and 7 run TestLogin first after mode 2's failed retry.

Lifecycle runs on this host hit "Indexer deadline exceeded" under heavy disk pressure
(Linux PSI `io full avg60` near 80%), with stalls of 25-60 s in fsync-backed session
and config saves rather than in request spacing. Running the harnesses with `TMPDIR`
on tmpfs separates that noise: the cookie and form lifecycles then pass in full.

## Current state

`tests/cookie_auth.rs` was reserved by the integrator during this change, so its two
expectation updates were handed over as a patch rather than committed here: in
`aidoru_ajax_table_requires_login_preflight_and_its_own_response_marker` modes 1 and 5
send one request, and in
`declared_user_agent_reaches_positive_login_search_redirect_and_download_only_same_origin`
the `/redirect` search has no `/index.php`, `/validated` preflight. Verified with
the patch applied: 19 passed.
