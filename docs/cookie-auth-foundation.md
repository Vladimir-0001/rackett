# Supplied-cookie private tracker support

Source pin: Jackett/Jackett `fe8876d21b93dfda84eb12bd24e1c72e56fb8736`.
The former inactive foundation is now integrated through the engine, management
API, search transport and opaque download tickets. Musopia was the first admitted
cookie-login preset; the 57-provider NexusPHP family
now uses the same validated session path. Its pinned YAML is unchanged. This is fixture-backed support,
not evidence of live Musopia availability; no account or private tracker was used.

## Supported behavior

Add Musopia from the catalog and enter the Cookie request header from an already
signed-in tracker session. The setting is secret and omitted from management
reads. Saving validates the configuration without contacting the tracker. An edit
on the same origin preserves an omitted cookie; an origin change requires an
explicit replacement and otherwise fails without changing the saved configuration.

Each configured source owns a mutable cookie session and its own client, request
gate and origin. Every uncached search first requests `login.test.path` and requires
its selector. The search page must also contain that selector. Downloads recheck
login before fetching the torrent. Requests start with the supplied Cookie header,
then use the source's refreshed cookies after exact-origin and path validation;
redirects never forward them to another origin. Search
results expose local opaque download tickets, not cookie or passkey values. Edits,
disabling, deletion and restart retire old tickets. An edit with the same source,
preset, base URL and seed cookie keeps the refreshed session; changing any of those
uses a separate session identity.
Successful cached searches retain the existing TTL behavior: a cache hit does not
revalidate login, but downloading a cached result does.

Musopia's synthetic fixtures exercise its real search path, category inputs,
freeleech/sort settings, title fallback, size, seeders/peers, date, and authenticated
torrent download. A zero-row page with the login selector is accepted as empty;
there is no independently captured real empty-page fixture or additional container
claim. A page which imitates the login selector can still pass this presence test.

## Source evidence and conservative limits

Pinned `CardigannIndexer.cs` `DoLogin` lines 810–813 copies the `cookie` setting.
`TestLogin` lines 892–950 requests the declared path, permits at most one same-domain
redirect and requires the declared selector. `CheckIfLoginIsNeeded` lines 952–997
treats redirects or missing login markers as lost login. Upstream can relogin;
Rackett returns an error without replaying the search.

`HttpWebClient2.cs` lines 113–125 and `CookieUtil.cs` implement cookie merging,
expiry and deletion. Rackett now keeps response updates in a bounded, source-local
cookie store. Updates from login, redirects, search and download apply before the
response is processed. Same-name/domain/path updates replace previous values;
Max-Age takes precedence over Expires, expired values are removed, and the upstream
expired sentinel deletes the cookie. The first server update to a pasted cookie
adopts its server-provided scope so the old unscoped credential cannot reappear.
The cookie crate parses names and attributes; date tokens follow
[RFC6265 section 5.1.1](https://www.rfc-editor.org/rfc/rfc6265#section-5.1.1).
The last valid signed 64-bit Max-Age follows pinned `CookieUtil`.

The store enforces path boundaries and Secure/prefix rules, accepts only domains
matching the configured host, and still sends to exactly one scheme/host/port.
It is not a shared browser jar and does not perform public-suffix or cross-site
navigation. Its path scoping is stricter than upstream's flattened cookie header.
Limits are 64 cookies, 16 KiB total state/response updates and 4 KiB per Set-Cookie.
Oversized or invalid-header batches fail without partially changing the store.
Other syntactically invalid or foreign-domain cookies are ignored.

Supplied headers still require nonempty RFC6265 cookie pairs; quoted/malformed
values, duplicate names, controls and the expired sentinel are rejected. Response
cookies may have empty or quoted valid values. A `Refresh` response header and
recognized Cloudflare header/body markers remain errors. This private-cookie path
does not use FlareSolverr or form/CSRF login.

Engine-managed refreshed cookies now survive restarts. The operator's configured
seed is unchanged. Private snapshots live in `<config path>.sessions/`, named by
a SHA-256 identity covering source ID, preset, canonical base URL and seed digest.
They are bounded to 64 KiB and validate cookie scopes and format on load. New
directories use mode 0700 and files mode 0600 on Unix. These files contain secrets;
include the directory in private backups, never publish it. Empty snapshots preserve
deletions, so restart cannot resurrect an invalidated seed. Invalid or unreadable
snapshots fail before network access instead of silently reverting to that seed.

Updates are written through a synced temporary file, atomic rename and directory
flush before processing the response. A blocking worker retains the owned session
lock if a request is cancelled. Unchanged source identities share that lock across
runtime edits, preventing an old request's late save from overwriting a newer one.
A failed save keeps the refreshed cookies dirty in memory and returns a fixed
storage error; subsequent requests retry persistence before any network activity.
There is no HTTP replay or rollback to old credentials, including after a rename
whose directory flush failed. Runtime shutdown can still leave either complete
snapshot; it cannot guarantee a server-issued rotation reaches disk if the process
dies before the write starts.

The repository assumes one process per config path, matching the single Rackett
instance deployment. Old identity files remain private after credential changes or
source deletion; they are not automatically reused by a different identity. To
erase them or deliberately reset to a seed, stop Rackett and remove the relevant
snapshot (or its session directory). Direct standalone `CookieSession::new` callers
remain memory-only; the engine binds persistent storage. This foundation does not
admit additional presets without their provider fixtures.

The compiler accepts only the exact supplied-cookie login shape, a literal bounded
login-test path and CSS selector, UTF-8 HTML GET searches, and one declared secret
cookie setting. Duplicate mapping keys, login errors and captcha blocks remain
unsupported. Search headers ([session search headers](session-search-headers.md)),
JSON/POST searches ([session search shapes](session-search-shapes.md)), uniform
path-level and root `followredirect`, `download: {method: post}` and `:contains` test
selectors ([cookie-session gaps](cookie-session-gaps.md)) were modeled later. Only explicitly fixture-validated `COOKIE_IDS` entries can consume
the plan's login blocker; other private presets stay gated. Authenticated requests
now use the same delay calculation as public requests: the greater of the pinned
minimum and configured delay, bounded at 60 seconds. Musopia has no pinned delay.

Login and search pages share scripting-off HTML parsing, 8 MiB/100k-node/depth-128
bounds, and inert-template-aware selector checks. A logout link inside a template
cannot authenticate a page. Positive selectors made only from element names,
IDs, classes and ordinary attributes can prove login outside an HTML `select`.
The select element and its contents are excluded from that check, so an ordinary
category dropdown no longer rejects an otherwise authenticated page. All 90
schema-compatible, compiling cookie definitions without another blocker use
selectors in this subset; this check alone does not admit those definitions.

Malformed selects still reject the whole authentication page. This matters beyond
the select subtree: AngleSharp enters raw text on `<xmp>` in a select whereas
html5ever drops the tag, so markup after `</select>` can have a different meaning.
The auth parser therefore captures unexpected-token errors from html5ever's
`InSelect` and `InSelectInTable` modes before matching anything. The exact diagnostic
suffix is tied to the pinned html5ever version and covered by regression tests;
diagnostic contents are neither retained nor returned. Shadow-root documents and
structural/text selectors retain the original whole-document guards. Ordinary
search extraction and `search.error` behavior are unchanged. Noscript is parsed
normally. This is a bounded compatibility rule, not full DOM equivalence.

The transport has explicit same-origin redirect limits (one for login, none for
search unless every search path declares `followredirect`, then five bare hops; five
for download and download detail pages), 8 MiB HTML/32 MiB download byte caps, parser concurrency
limits, per-source request spacing and deadlines. Torrent content validation is
still performed by the protocol download handler. All authentication/transport
errors are static and contain no cookie, response content or search query.

## Validation

`tests/cookie_auth.rs` covers schema rejection, cookie syntax, origin isolation,
redirect limits, byte/deadline/rate limits, scripting-off parsing and inert/unsupported
DOMs, positive login markers around ordinary forms, malformed-select raw-text
traps and all candidate cookie-family login selectors. `tests/musopia_runtime.rs`
covers management create/restart/test/search,
redaction, actual request parameters, caching, cookie replacement, atomic rejection
of origin changes without new credentials, ticket expiry, authenticated downloads,
expired sessions, rotating/deleted/scoped cookies across search and download,
source isolation, Refresh and header/body challenges. Cookie-store unit tests pin
expiration dates, Max-Age precedence, default paths, empty values, prefixes,
bounded atomic updates and redaction. These use only
loopback fixtures and dummy credentials. Existing download epoch/concurrency tests
continue to apply. No live source was added to the deployment configuration.

The UI displays `info_cookie` as instructions rather than an editable setting.
The pinned vendor definitions and goldens remain unchanged.
