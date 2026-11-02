# API origins: searches on a declared API host

Some bundled public definitions search an API host that is not their site. Their
search path is an absolute URL, either templated from a setting or written out:

| ID | Search path | Default host |
| --- | --- | --- |
| yts | `https://{{ .Config.apiurl }}/api/v2/list_movies.json` | `movies-api.accel.li` |
| thepiratebay | `https://{{ .Config.apiurl }}/q.php?...` or `/precompiled/data_top100_*.json` | `apibay.org` |
| animetosho-xyz | `https://{{ .Config.api-url }}/api/torznab` | `feed.animetosho.net` |
| polishtracker-api | `https://api-test.pte.nu/api/v1/torrents` (literal) | not admitted, see below |

Before this change every planned request, redirect and final URL had to stay on the
configured site origin, so all four were refused before sending. Rackett now lets these
searches use one extra origin under a narrow, per-ID rule, without weakening the site
origin policy for anything else.

## Upstream behaviour (Jackett `fe8876d2`)

- `resolvePath(path, currentUrl)` is `new Uri(currentUrl ?? new Uri(SiteLink), path)`
  (`CardigannIndexer.cs:1436`), so an absolute search path is used as is. The search
  URL is that path, rendered with `WebUtility.UrlEncode` on every substituted value
  (`:1543`), plus the query string (`:1589-1592`).
- Upstream sends the request with the indexer's stored cookie header and the rendered
  search headers (`:1605-1607`; `BaseIndexer.cs:598-621`, `Cookies = cookieOverride ??
  CookieHeader` at `:607`). It follows a redirect only when the path sets
  `followredirect` (`:1609-1612`).
- `download` and `details` values resolve against the search URL, not the site:
  `resolvePath(value, searchUrlUri)` (`ParseFields`, `:2214-2241`). Posters too (`:2423`).
- A release with an infohash and no magnet gets a public magnet
  (`BaseIndexer.cs:209-211`, public definitions only).
- A JSON rows selector is rendered with the configuration variables
  (`applyGoTemplateText(rowSelector)`), split at the first `:` into a token path, and
  each row is kept only if the remaining `:has`/`:not`/`:contains` filters match
  (`JsonParseRowsSelector` and `JsonParseFieldSelector`, `:2491-2562`; the filter regex
  is at `:73`). The Pirate Bay uses this for its optional uploader filter:
  `${{ if .Config.uploader }}:has(username:contains({{ .Config.uploader }})){{ else }}{{ end }}`.

## Rackett's rule

1. **Per-ID allowlist.** `builtin::API_ORIGIN_IDS` (`src/builtin.rs`) lists the
   definitions that may use an API origin: `animetosho-xyz`, `thepiratebay`, `yts`. It
   admits no runtime by itself; each ID still needs `READY_IDS` and fixtures, as the
   local `tokyotosho` and infohash policies do.
2. **The origin comes from the definition.** `builtin::api_origin` runs when the runtime
   compiles. Every declared search path must start with `http://` or `https://`. Its host
   must be a literal or exactly one `{{ .Config.name }}`, and that setting's configured
   or default value must be a non-empty host that the rendered URL really uses. The
   requests are rendered with an empty query, so no query value can choose the host. All
   paths must name one origin. If it equals the site origin, nothing changes. Because
   upstream URL-encodes substituted values, a port, credentials, a path or an empty value
   in the setting cannot change the URL structure. These all fail to compile.
3. **Requests.** Each planned request must be on the site origin or on that API origin.
   The whole batch is checked before anything is sent (`request_base`,
   `src/builtin_runtime.rs`). Anything else, including another host, fails as before.
4. **API requests are cookie-free and have no session** (`Transport::fetch_api`). They
   carry only the plan's own validated headers. A plan `Cookie` header is refused, and
   no saved cookie, solver clearance, cookie/form session, API session or `login: get`
   probe applies. Upstream would send its stored cookie header. An admitted public
   source has none, so dropping it changes no admitted behaviour. Challenges are never
   solved on the API host: a Cloudflare challenge is an error.
5. **Redirects are pinned to the request's own origin.** `send` checks each hop, and
   the final URL, against the API origin. A redirect to any other origin, including
   the site, is rejected without being followed.
6. **Release links stay downloadable.** Links that resolve against an API search URL
   are kept only if they are magnets or on the site origin. Any other HTTP link fails
   the search ("Release download link must remain on the site origin"), so no ticket is
   ever issued for the API origin. Downloads and detail pages keep the existing
   site-only checks in `Engine::download` and `Transport`. Details and posters are only
   presented, never fetched.
7. **TLS.** Definition `certificates` pins apply to the site host only, so an API host
   always gets normal WebPKI verification.

## JSON row selector values

`Template::render_json_selector` (`src/cardigann_exec/template.rs`) renders a JSON rows
selector that substitutes text settings. The compile-time checks are unchanged: the
value must come from a text setting, be guarded by its own `if`, and fill the whole
`:contains(...)` argument, and every variant must parse as a JSON selector. At runtime the
value is literal `:contains` text, as upstream, so CSS identifier rules do not apply.
Parentheses and control characters are rejected, because with them the upstream regex
and Rackett's parser could split the selector differently. The rendered selector must
parse. A blank value selects the `else` branch (`IsNullOrWhiteSpace`).

## Admitted IDs

| ID | Status | Fixture |
| --- | --- | --- |
| yts | admitted (`READY_IDS`) | `public-batches/batch16.json`, `public-layouts/yts.json` |
| animetosho-xyz | admitted (`READY_IDS`, container `channel`) | `public-batches/batch16.json`, `public-layouts/animetosho-xyz.html` |
| thepiratebay | on `API_ORIGIN_IDS`, waits for the `$`-rooted JSON path fix | `public-batches/batch16-thepiratebay.json`, `public-layouts/thepiratebay.json` |

The Pirate Bay has no download field. Its releases use the infohash-to-magnet policy
(`thepiratebay` was added to that list in `search_source`). Its count selector is
`$[0].id`; `$.x` and `$[0].x` select from the token passed in, as Newtonsoft does. Upstream's no-results body is a single
row with `id` `"0"`, which the count treats as empty. Its case is in
`public-batches/batch16-thepiratebay.json`.

## Left blocked

- **polishtracker-api.** Its search path is a literal API host, which this rule could
  support. But it is a private API-key definition. The `API-Key` header and the
  credential would go to `api-test.pte.nu`, and its `download` is the relative
  `/download/{{ .Result._id }}`, which upstream resolves against the search URL, so the
  `.torrent` lives on the API host and needs the key. Rackett's tickets and downloads are
  site-only by design (rule 6). Supporting it would mean a ticketed, credentialed
  download origin, which is a separate policy decision. It is not on `API_ORIGIN_IDS`.

## Fixture harness

The public harness (`tests/public_layout_runtime.rs`) serves the API host with a second
mock server: the same router and fixture state over loopback TLS. A `:port` cannot be
used, because upstream and Rackett URL-encode the substituted host (`127.0.0.1%3A8443`
is not a valid host) and the definitions hard-code `https://`. So a case names its host
setting in `api_setting` and sets it to a synthetic `*.fixture.test` host. The
lifecycle generates a throwaway self-signed certificate for that name and calls
`rackett::engine::register_fixture_api_host`. Engine clients created afterwards in that
test process resolve the name to the loopback listener and trust that certificate. The
hook is `#[doc(hidden)]` test support: only code can call it, never configuration,
environment or a request, and it accepts only loopback addresses.

The fixture checks the `Host` header. Searches must arrive on the API host and
downloads on the site. The usual lifecycle (cache, download, edit, restart, error,
challenge and off-origin redirect modes, recovery) runs unchanged otherwise. Two more
case options:

- `error_body`: the mode-2 error body, for sources where the default JSON error would
  be "no results". yts has `missingAttributeEqualsNoResults`, so it serves HTML.
- `@@SITE@@` in a fixture page is replaced by the site base, for API responses that
  carry absolute site links: Anime Tosho's `link` and `enclosure`. Relative links
  would resolve against the API host.

Unit tests (`src/builtin_runtime.rs`):
`api_origin_searches_stay_on_their_origin_and_keep_links_on_the_site` covers a
cookie-free API search with a site link, a relative (API-origin) link rejected, a
redirect from the API origin to the site rejected without being followed, a plan
`Cookie` header refused, the same plan refused without an admitted origin (not on the
allowlist), and a third origin refused.
`api_origins_come_from_listed_absolute_templates_and_settings_only` covers default and
configured hosts, the site host, and port, credential, path and empty values.

## Validation (2026-11-01)

- `cargo test --lib`: 188 passed.
- `cargo test --test cardigann_config_selectors`: 12 passed.
- `cargo test --test public_layout_runtime`: 7 passed, 1 ignored (full run, 124 s),
  including the yts, animetosho-xyz and thepiratebay lifecycles over loopback TLS.
- `cargo test --test cookie_auth`: 19 passed.
- After integration, every test target passes, including `builtin_runtime` and
  `nexusphp_cookie_runtime`.
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` are clean.

## Limits

- One API origin per definition. All search paths must name the same origin.
- Live Anime Tosho could not be checked from the development host, because
  `feed.animetosho.net` returned a Cloudflare challenge. The site's own pages link
  torrents as `/download/<id>/torrent` on the site origin, and the fixture models the
  feed's `enclosure` that way. If the live feed returns torrent links on another host
  (for example `storage.animetosho.net`), searches fail closed with the site-origin
  error rather than issuing unusable tickets. Preferring the row's infohash magnet in
  that case, as `tokyotosho` prefers its magnet, would be a further local policy.
- An API host that needs a challenge solver is unsupported.
