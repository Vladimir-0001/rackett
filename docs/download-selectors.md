# Download selector resolution

56 pinned definitions declare a `download:` block: the release link from search is a
detail page, and the real torrent or magnet must be found on it. Rackett now runs the
most common shape, `download.selectors`, through the same per-source transport as
search.

## Behaviour (pinned `CardigannIndexer.cs:2083-2185`)

- The ticketed release link is fetched: plainly for public sources, through the
  authenticated session for cookie, form and credential-POST sources.
- Each selector is tried in declaration order. It takes the **first** element the
  CSS selector matches in document order, then its `attribute` (or text), then its
  `filters`. A selector that doesn't match, or whose element lacks the attribute,
  moves on to the next one, as upstream's exception handling does.
- The value is resolved against the detail page URL. A magnet (`btih` only, no
  controls or whitespace) is returned as a magnet download. Any other link must be
  same-origin. A cross-origin candidate is skipped, so external hosts are never
  fetched.
- With `testlinktorrent` (default true), a downloaded body that is non-empty and
  doesn't start with `d` moves on to the next selector. Downloads are still
  validated as torrents before being served.

Differences from upstream: the detail page is fetched once and shared by all
selectors (upstream refetches per selector), and an accepted torrent is downloaded
once (upstream fetches it to test, then again to serve). Both mean fewer requests
to the site, not different results.

## Admitted subset

`src/builtin_runtime/download.rs` accepts `download:` with an optional `before` and at
most one of `selectors` or `infohash`:

- at most 8 selectors, each with `selector`, an optional `attribute` and optional
  `filters`;
- selector text may contain templates and may be `:root` (see below);
- `search.headers` only for public definitions (no `login:` block); a `download.headers`
  key, `download.method` and per-selector `usebeforeresponse` stay blocked.

### Templated selectors (`MatchSelector`, pinned `CardigannIndexer.cs:2048-2081`)

Upstream renders each selector with the download variables before querying the page
(`:2050`, and again for logging at `:2148`). Rackett renders it with the source context
plus `.DownloadUri.*` and parses the result with the bounded CSS engine at match time.
A rendered selector that does not parse, or a filter that fails, is an exception inside
the per-selector `try` (`:2150-2180`): that selector yields nothing and the next one is
tried. limetorrents and torrentdownloads use this for their `primarydownloadlink` /
`fallbackdownloadlink` select settings (`a[href*="{{ .Config.primarydownloadlink }}"]`).
With the default `magnet:` primary the magnet is returned; the iTorrents fallback is
cross-origin and is skipped, never fetched. A 1337x definition altered away from the
exact [`DetailMagnet`](../src/builtin.rs) shape now falls through to this general
resolver instead of being blocked; the same-origin and magnet checks still apply.

### Search headers (`:2094`, `:2144`, `ParseCustomHeaders` `:2192-2207`)

Upstream sends `Download.Headers ?? Search.Headers`, rendered with the download
variables and using only the first value of each header, on the detail page fetch,
the `before` request, the test-link fetch and the final download. Rackett renders
`search.headers` the same way and attaches them to the detail page and torrent
requests of a public source (catorrent, damagnet, skidrowrepack send
`Referer: {{ .Config.sitelink }}`). Login sessions have no way to send custom headers,
so a definition with a `login:` block and search headers stays blocked. Header names
follow the search plan's token rule; values are bounded and control characters fail.

### `before` on public sources and `usebeforeresponse` (`:2096-2107`, `:2118`)

For a public definition the `before` request is a plain-transport GET to
`resolvePath(path)`, i.e. against the site link (`:1436`), with the release link as
Referer (`HandleRequestAsync` `:1971-2011`). POST `before` blocks and `before` together
with search headers stay blocked on public definitions (no pinned definition needs them;
upstream would also send the headers alongside the Referer).

`infohash.usebeforeresponse: true` with a `before` block selects the hash and title on
the `before` response instead of fetching the release link (`:2118`). Without a `before`
block the link is fetched as usual. magnetdownload requests
`api/json_info?hashes=<id>` and reads the JSON reply with `:root` text and regexps;
AngleSharp parses the JSON as an HTML document whose root text is the whole body,
which the bounded parser reproduces. `usebeforeresponse` on login sources stays blocked
(the session discards the `before` response).

`before` paths now resolve against the site link for login sessions too; previously
they were joined to the release link, which only differed for nested release paths.

`infohash` (hash and title selectors on the detail page) returns upstream's
`InfoHashToPublicMagnet(hash, title)`: the hash, the WebUtility-encoded title and the
pinned list of 10 public trackers. A missing hash or title gives no download.

`before` (`HandleRequestAsync`) sends one GET or POST, typically a "thanks" form,
through the logged-in session, with the release link as Referer. Its path and inputs
are templates that see `.DownloadUri.*` (from the release link, first value of a
repeated query key) and `.Config.*`. GET inputs join the query with the definition
charset; POST inputs become form fields. The response is ignored, and same-origin
redirects are followed. Selectors or infohash then run as above. Without them, the
release link itself is downloaded, as upstream does. `pathselector` and
`usebeforeresponse` stay unsupported. Form harness: `before_path` and `before_fields`.

`download.method` stays blocked: hdsky (`download: {method: post}`) needs the release
link POSTed through its cookie session (`:2109`, `base.Download(link, method, ...)`
`:2189`), and
the cookie session (`src/builtin_runtime/auth.rs`) only downloads with GET. Header-bearing
login definitions (pier720) stay blocked. The 1337x `DetailMagnet` policy is unchanged
for the exact pinned 1337x block.

The resolver is compiled from the pinned definition on demand
(`builtin::download_selectors`) when a ticket is downloaded. Form and credential-POST
plans now accept a definition whose download block compiles in this shape.

## Test harness

The public harness also accepts `expected_size` for definitions with a fixed size,
and per-case `seeders`/`peers`. For `date: now`, `published` is the harness clock
`2026-10-29T12:00:00Z`.

The form, cookie and public lifecycle cases accept an optional `detail` object:

```json
"detail": {"html": "<a href=\"download.php?id=42&amp;passkey=x\">get</a>", "resolved": "/download.php?id=42&passkey=x"}
```

The fixture serves `html` at `download_path` (in the definition's charset), and the
torrent at `resolved`. With `"before": "/api/json_info?hashes=42"` (public harness) the
`html` is served at that request instead and the release link must not be fetched
(magnetdownload). In public `expected_headers`, `$SITELINK` stands for the fixture site
link, so `{"referer": "$SITELINK"}` checks `Referer: {{ .Config.sitelink }}` on every
search, detail and torrent request. If `resolved` is a `magnet:` URI, the download must return
exactly that magnet. Unit tests in `download.rs` cover the admitted and rejected
pinned shapes, first-match document order, misses and the page byte budget.

## Now compilable (fixtures pending)

- **Public (21):** blueroms, byrutor, crackingpatching, ehentai, freejavtorrent,
  mactorrentsdownload, mixtapetorrent, mypornclub, pctorrent, plugintorrent,
  pornrips, pornxlab, rintornet, sosulki, torrentdownload, torrentgalaxyclone,
  torrentproject2, vsthouse, vstorrent, vsttorrents, xxxclub.
- **Cookie (3):** fappaizuri2fa, hdarea, u2.
- **Form/POST (7):** chdbits, dimeadozen, dxp, fappaizuri, ncore, proporno,
  torrentdd. hellashut, thebrothers and thecrazyones also need unsupported
  form-login properties.

## Admitted with fixtures (public batch 14)

catorrent, damagnet, limetorrents, magnetdownload, skidrowrepack and torrentdownloads
(`tests/fixtures/public-batches/batch14.json`). torrentdownloads' path-level
`followredirect: true` and nested `:has()` rows already compile; its only remaining
blocker was the templated download selectors. None of them is infohash-only: every
release has a detail link, so no infohash allowlist entry is needed.

