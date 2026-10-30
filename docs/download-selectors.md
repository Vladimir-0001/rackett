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

`src/builtin_runtime/download.rs` accepts only `download: {selectors: [...]}`:

- at most 8 selectors, each with `selector`, an optional `attribute` and optional
  `filters`;
- no templates in selector text, no `:root`;
- no search or download `headers` (upstream sends them with download requests).

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

`method` blocks, templated selectors (limetorrents) and
header-bearing definitions (catorrent, skidrowrepack, pier720) stay blocked.
The 1337x `DetailMagnet` policy is unchanged.

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
torrent at `resolved`. If `resolved` is a `magnet:` URI, the download must return
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
