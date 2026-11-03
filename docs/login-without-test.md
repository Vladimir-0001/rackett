# Logins without a `login.test`

Some Cardigann definitions declare no `login.test` block. Cookie and form sessions used
to reject them ("Unsupported cookie authentication schema", "Form login needs a
positive test"). They now follow the pinned Jackett behaviour (commit `fe8876d2`,
`src/Jackett.Common/Indexers/Definitions/CardigannIndexer.cs`) for both a missing
test and a test with a path but no selector.

## Upstream evidence

- `TestLogin()` (`:892-949`) returns at `:896-899` when `Login.Test == null`, so no
  validation request is sent. With a test it GETs the test path (`:902-904`), follows one
  same-domain redirect (`:907-912`), fails on a remaining redirect (`:914-933`), and
  checks the page only when `Test.Selector != null` (`:935-946`).
- `CheckIfLoginIsNeeded()` (`:951-997`): a search redirect always means relogin
  (`:953-975`). Without a test it returns false otherwise (`:977-980`). With a test but
  no selector it also returns false after the redirect check (`:984`).
- `ApplyConfiguration` (`:1104-1112`) and `Relogin` (`:874-890`) both call `DoLogin`
  and then `TestLogin`, so a missing test also means no request after a login or relogin.
- `DoLogin`'s `post` (`:574-597`), `form` (`:598-809`) and `cookie` (`:810-813`)
  branches do not read the test.

## Semantics in Rackett

`CookiePlan` and `FormPlan` hold `test: Option<LoginTest { path, selector:
Option<String> }>`. An absent value is the pinned behaviour, never a default marker.
Definitions that declare a test keep the earlier behaviour and session identity.

- Cookie sessions: without a test, `validate` sends nothing. Searches return the page
  without a marker check. A search redirect still fails the search: supplied cookies
  cannot reauthenticate, and the redirect is not followed.
- Form and credential-POST sessions: without a test, a stored session cookie for the
  source origin counts as logged in, with no probe request. Otherwise the session logs
  in (landing page, submit, error selectors) and sends no test request afterwards.
- Search expiry: a redirect triggers one relogin and one retry of the same request.
  A page with no login marker is passed to the plan, so an HTML search returns an empty
  result instead of a relogin. HTTP 401 triggers relogin only when the definition
  declares a test selector; otherwise it is an HTTP error.
- A test with a path but no selector is still requested (one redirect followed, 2xx
  HTML required) but its body is not checked. No bundled definition uses this form today.
- Sessions with a test send it only when they are not yet validated; see
  [session-validation.md](session-validation.md).

## Harness flag

Cookie (`tests/cookie_layout_runtime.rs`) and form (`tests/form_layout_runtime.rs`)
cases accept `"no_test": true`. `login_path` is then unused: set it to a path the
source must never request.

- First search: the cookie harness expects one request per search path, with no
  preflight. The form harness expects landing (unless a credential POST), submit and
  search requests only.
- Mode 2 (a marker-less sign-in page): with no test, no relogin happens. HTML searches
  return no rows and no error; JSON searches fail to parse.
- The cookie harness adds mode 5 for these cases: a search redirect returns an error
  and is not followed. The form harness's existing mode 6 (one search redirect) checks
  the single relogin.
- The cookie harness extracts through `Plan::extract_response` with HTTP 200 metadata.
  superbits declares `noResultsMessage: ""`, which `Plan::extract` rejects without
  transport metadata. Other cases behave the same either way.

Unit tests: `builtin_runtime::auth::session_tests::{cookie_login_without_test_sends_no_validation_and_rejects_search_redirects,
form_login_without_test_relogs_only_after_a_search_redirect}` and
`form_auth::tests::absent_or_selector_less_login_tests_are_explicit_not_defaulted`.

## Admitted

- Cookie (`tests/fixtures/cookie-batches/batch3.json`): rousi-pro, superbits (JSON API
  searches).
- Form (`tests/fixtures/form-batches/batch17.json`): immortuos, zappateers (UNIT3D
  login with a `_token` selector input).

Allowlist admission (`READY_IDS`, `COOKIE_IDS`, `FORM_IDS` in `src/builtin.rs`) is left
to the integrator; run `python3 tools/admit_form_batches.py` and add the cookie IDs.

A scratch census (`CookiePlan`/`FormPlan::compile` over `builtin::catalog()`) found
that these four, plus comicat and miobt, were the only non-`get` logins without a test. No bundled definition declares a test path without a selector. No other
definition started compiling.

## comicat and miobt

`FormPlan` compiles both. They declare no settings and send the literal input
`visitor_test: human`, so the form harness gained `no_credentials` (no username/password
settings; the password-change step is skipped, see fixture-harness.md).
Their rows have no download link: the magnet comes from the `infohash` field. That needs
the infohash allowlist in `src/builtin_runtime.rs` (`resolve_results`, next to `btdirectory`
and `magnetcat`), and the harness' `download_path: "magnet"` case. Both are admitted in
`tests/fixtures/form-batches/batch24.json`, with the two IDs added to that allowlist.

miobt's `今天`/`昨天` branch appends ` +08:00` before `fuzzytime`. Pinned `FromUnknown`
parses `Yesterday 22:50 +08:00` through `DateTime.Parse(time).TimeOfDay`, which converts to
the server's local time. Until 2026-11-01 Rackett's bounded parser rejected the offset, and the
optional field fell back to `now`. It now reads the clock in UTC, Rackett's convention for
upstream's local time: `Yesterday 22:50 +08:00` is yesterday at 14:50 UTC (see
[cardigann.md](cardigann.md#fuzzy-dates-day-month-year-and-relative-day-offsets-2026-11-01)).
The fixtures use miobt's `yyyy/MM/dd` form. comicat's `昨天` branch has no zone and is
unaffected.

## Validation (2026-10-30, host load average 20-30 on 8 cores)

With the four IDs admitted locally in `src/builtin.rs`:

- `cargo test --lib`: 175 passed, including the three unit tests above.
- `--test cookie_auth` 19 passed, `--test form_auth` 14, `--test cookie_challenge` 2,
  `--test builtin_runtime` 10.
- `--test cookie_layout_runtime`: 5 passed, including the rousi-pro and superbits
  lifecycles.
- `--test form_layout_runtime`: immortuos and zappateers passed their lifecycle in
  all three runs. Each run failed on a different set of sites with pinned 2-5 s request
  delays and a test login, almost all with "Indexer deadline exceeded" (run 1:
  totheglory, bootytape, pttime, haitang, crabpt, magico, newstudiol; run 2: crabpt,
  pttime, haitang, wintersakura, gay-torrents, jpopsuki, newstudiol; run 3:
  diablotorrent, totheglory, gay-torrents, newstudiol). This is the load sensitivity
  recorded in commit `9bb5497`. Run on their own with a local case filter, newstudiol,
  diablotorrent and totheglory passed. gay-torrents (five search paths) also hit the
  deadline with the unchanged `47d16e7` engine files at the same load, so the failures
  predate this change. Sessions with a test send the same requests as before.
- `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings`
  are clean.

## Limitations

- Without a test, Rackett cannot tell a logged-out page from an empty result. Upstream
  has the same limitation.
- A form session whose login sets no cookie at all logs in again whenever it is not
  validated (first use after a restart, or after a failed search), because the
  stored-cookie check is then the only "logged in" signal.
