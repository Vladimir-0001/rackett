# FlareSolverr and saved-cookie transport

1337x, EZTV and Tokyo Toshokan are admitted challenge-capable runtime presets. Saved-cookie transport and the
configured FlareSolverr fallback are shared by searches and lazy 1337x downloads.

## Deployment

```sh
RACKETT_FLARESOLVERR_URL=http://flaresolverr:8191/
```

If the solver's browser and Rackett reach a tracker using different public IPs,
the browser's clearance may be rejected. In one observed setup the host used IPv6,
while the containerized solver used IPv4. Replaying the same clearance and
user agent produced a 403 challenge over IPv6 and an ordinary 200 result over
IPv4. An opt-in setting aligns the source client's address family:

```sh
RACKETT_FLARESOLVERR_SOURCE_IP_FAMILY=ipv4
```

Values are `auto` (also unset/empty), `ipv4` or `ipv6`. Invalid values fail startup
with a static error; values are read once and changing them requires a restart.
Only admitted challenge sources (1337x/EZTV/Tokyo Toshokan) with a configured solver are pinned.
Native adapters, other bundled sources and the HTTP connection to the solver
retain their previous behavior. No source settings, cookies or readiness gates
change. A forced family will fail for a tracker that cannot be reached over it;
there is no fallback to the other family. Matching family alone does not guarantee
matching public IP if a proxy, VPN or remote solver uses another egress route.
The operator must align the actual routes; Rackett does not discover public IPs
by contacting an external service.

- Unset or empty disables the fallback; there is no implicit endpoint.
- Accepted: HTTP(S), a host, path `/` or `/v1`, and no credentials, query or
  fragment. Requests go to `/v1`. An invalid value stops `Engine::new` (startup)
  with a static message that does not echo the value.
- The value is read once per process. Restart to change it. It needs no settings UI
  and no configuration-file field.
- The solver client uses no proxy, no redirects and no retries.
- One solve is bounded by `min(60000 ms, remaining search deadline − 1 s)`. If less
  than 1 s is left, no solver request is sent. The search deadline is the global
  `request_timeout_seconds` (default 20, max 120). Solves often take several
  seconds, so raise it, for example to 60.

## FlareSolverr API evidence

This was read from the installed image `ghcr.io/flaresolverr/flaresolverr:latest`
(`/package.json` version **3.5.2**, files `/app/dtos.py`,
`/app/flaresolverr_service.py` and `/app/flaresolverr.py`). The image was inspected
in a throwaway `--network none` container. The running service was not touched.

- `POST /v1` takes `{cmd, url, maxTimeout, cookies?}`. `cmd: request.get` rejects
  `postData`. A missing or nonpositive `maxTimeout` becomes 60000. v2 removed the
  `headers` and `userAgent` request fields.
- When `cookies` is given, the browser loads the URL, adds the cookies and loads the
  URL again.
- The reply is `{status: "ok"|"error", message, solution, startTimestamp,
  endTimestamp, version}`. Errors return HTTP 500.
- `solution` holds:
  - `url`: `driver.current_url`
  - `status`: always 200; the source has a todo because Selenium does not report it
  - `headers`: always `{}`
  - `response`: `driver.page_source`, the rendered DOM rather than raw bytes
  - `cookies`: WebDriver cookies, each with name, value, domain, path, secure,
    httpOnly, expiry and sameSite
  - `userAgent`
- The upstream challenge titles are "Just a moment..." and "DDoS-Guard", plus the
  `CHALLENGE_SELECTORS` list.

## Behavior

**Saved cookie.**
- `Indexer.cookie` is admitted only for `builtin::CHALLENGE_IDS = [1337x, eztv, tokyotosho]`.
- Values must be strict RFC6265 pairs, reusing the cookie-auth validator. A
  malformed value fails with "Invalid cookie setting".
- Other bundled IDs, and `api_key` on any bundled source, keep the "generic
  credentials are unsupported" rejection.
- Bundled clients no longer receive the cookie as a reqwest default header. The
  `Source` composes it per request instead, and only for the configured origin.
- A PATCH that moves a bundled source to another origin clears the saved cookie
  unless the PATCH sends a new one. This matches the existing preset-secret origin
  policy.
- Reads still redact `cookie`.

**One Cookie header.**
- The plan's own Cookie pairs come first and always win. EZTV's pinned
  `layout=def_wlinks` and other layout pairs are never overwritten or duplicated.
- Solver clearance pairs come next, then saved pairs whose names are new.
- A cached clearance also replaces the user agent with the one the solver
  returned.
- With no saved cookie and no clearance, the plan's headers are sent exactly as
  before.

**Challenge recognition.** Only these count as a challenge, and only with a
configured solver, an admitted ID, GET, and a UTF-8 HTML response type:
- `cf-mitigated: challenge` (existing check), or
- HTTP 200, 403 or 503 whose first 256 KiB contains one of these markers:
  - `<title>Just a moment...</title>`
  - `window._cf_chl_opt`
  - `/cdn-cgi/challenge-platform/h/`

The `/cdn-cgi/challenge-platform/scripts/` probe that Cloudflare injects into
ordinary pages is not a marker. Other HTTP errors keep their existing messages, and
a 403 or 503 body is read (bounded at 8 MiB) only when a solver is configured.
POST is never replayed. JSON and Latin-1 plans keep the existing challenge error,
because a rendered DOM is neither raw JSON nor raw bytes.

**One solve.**
- At most one `request.get` is sent per original request. It sends the challenged
  same-origin URL and the plan's static Cookie pairs only, never the saved cookie,
  API key or other headers.
- It takes its own turn on the source's existing gate and delay and runs under the
  source's mutual exclusion.
- The rendered page is extracted directly, without a second origin fetch, so the
  existing redirect, status and empty-result policies see status 200, no refresh,
  and `redirected` if any earlier hop or the browser's final URL differs.

**Validation.** Each of these fails with a static local message and no row data:
- solver HTTP status other than 200, or a non-`ok` status
- an envelope larger than 16 MiB, or one that is malformed or missing its solution
- `solution.status` other than 200
- a final URL on another origin
- an empty response
- a rendered page larger than 8 MiB
- markers still present (an unsolved challenge)

An unsolved challenge is never returned as an empty result. Solver messages,
URLs, cookies, the user agent and bodies are not included in errors.

**Clearance state.**
- Clearance lives in `builtin_runtime::Source`, which the engine creates once per
  source runtime epoch. Source edits, deletion and restart therefore discard it, and
  no state is shared across sources or kept in a global jar.
- Only cookies valid for the exact origin at every path are kept:
  - the host matches; a leading-dot domain is narrowed to the host
  - path is `/`
  - `secure` is kept only over HTTPS
  - unexpired and a valid pair
  - at most 64 cookies and 4 KiB in total
- Clearance is cached only with a valid user agent (printable ASCII, at most 512
  bytes). It expires at the earliest cookie expiry.
- A failed solve clears any cached clearance.

## Reusable entrypoint for details fetch

The 1337x details resolver uses the same-origin transport, rate gate, epoch
cookies and solver as searches:

```rust
let mut transport = builtin_runtime::Transport {
    client: &r.client,
    source: r.source.as_ref().unwrap(),
    base: &base,
    gate: &mut gate,
    delay_ms,
    deadline,
};
let fetched: builtin_runtime::Fetched = transport.fetch(&built_get_request, true).await?;
// fetched.{status, redirected, refresh, url, content_type, body, rendered}
```

The resolver must hold the runtime's gate lock and stay on the runtime's origin.
`solvable = true` declares that the caller can parse rendered UTF-8 HTML.
`builtin_runtime::search` without `_source` keeps its old signature. It composes the
saved cookie but never solves.

## Tests

`tests/flaresolverr_transport.rs` uses a loopback origin and a loopback fake
FlareSolverr shaped like the 3.5.2 envelope. It runs the unchanged pinned EZTV
definition through `search_source`.
- Endpoint validation and default-off.
- Cookie admission and runtime compilation for 1337x and EZTV. Other IDs and `api_key`
  are rejected.
- Layout cookies and the saved cookie compose into one header. With no solver, the
  challenge error is unchanged.
- 403 `cf-mitigated` and a marker-only 503 each cause one solver POST. The test
  checks the exact request JSON (cookies limited to the layout pairs), that the
  rendered page is extracted with the right details base URL, the gate spacing, and
  that the follow-up uses the cached cookie and user agent without another solve.
- Ordinary Cloudflare pages with the jsd probe, plain 403/503/404 responses, and
  POST or JSON challenges never call the solver.
- Each invalid solver result fails without leaks or retries and leaves nothing
  cached: wrong origin, oversize page, oversize envelope, malformed JSON, missing
  solution, error/500, `solution.status` 403, challenge still present, empty page,
  unreachable solver.
- Clearance narrowing (domain, path, secure, expiry, invalid pair, missing domain).
  A plan-cookie name collision keeps the layout value. Source and epoch isolation,
  and an all-expired solution caches nothing.
- Too little deadline sends no solve. A slow solver is cut off at the deadline with
  `maxTimeout` inside it. A long deadline is capped at 60000. An origin that keeps
  challenging gets exactly one solve per request.

`tests/flaresolverr_env.rs` checks that an invalid environment value stops startup
without echoing it.

Forcing the fallback off makes 4 of the 8 transport tests fail.

## Remaining gaps

- **DOM parse.** HTML is now parsed with scripting disabled, including rendered
  solver pages. Ordinary nonempty noscript no longer blocks extraction. Other
  differences between rendered browser DOM and raw HTML remain possible.
- **Activation.** Neither source is activated. Live search, empty/error-page
  recognition and the saved configuration still need end-to-end validation.
- **Downloads.** The 1337x detail-to-magnet path uses this transport, including
  saved cookies and clearance reuse. An owner loopback test covers search → ticket
  → challenged detail page → solver → magnet redirect, followed by a second
  download using cached clearance. Direct torrent downloads retain their existing
  behavior. Cross-origin iTorrents fallback remains unsupported.
- **Clearance scope.** No check is made that a real `cf_clearance` stays valid with
  the solver's user agent from Rackett's TLS fingerprint. Cloudflare may reject it,
  which leads to one solve per request; this is bounded but slow.
- **Saved cookie.** The saved cookie (for example, an old `cf_clearance` bound to
  another user agent) is sent as-is on the first request. Its format must pass the
  strict pair validator: no trailing `;`, quotes, spaces in values or duplicates.
- **Upstream semantics.** FlareSolverr sessions, proxies, `request.post`, and
  DDoS-Guard or other non-Cloudflare challenges are not modeled.

## Public-tracker row policy

The 1337x/EZTV runtime skips individual HTML field failures, as pinned Jackett does.
This includes returning an empty result when every selected row fails: a captured
EZTV search had one row with seeders `-`, which Jackett skips. Offline `Plan::extract`
remains strict. Document bounds, selector scans, aggregate budgets and transport
failures still abort the response. A recognized HTTP-200 challenge is solved or
rejected before extraction; it is never treated as an empty result.
