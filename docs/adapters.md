# Native adapter contracts

Besides the bundled Jackett sites, Rackett can search five kinds of custom source. Add one from **Add source** in the web interface by choosing a custom template, then fill in its endpoint and, for JSON and HTML, a small field mapping. You can also connect an existing Jackett instance as a Torznab source.

All adapters accept only HTTP(S) endpoints without URL userinfo. Torrent download references may also be magnets. Bodies are streamed with an 8 MiB cap and must be UTF-8. Errors omit request URLs, response bodies and credentials. Requests include the indexer's configured cookie explicitly. The caller must provide a client that rejects redirects to a different origin; it owns timeout, retries, proxy settings, rate scheduling and cancellation. No adapter starts its own retry loop. Configured API keys must remain out of management read responses, logs and exports unless explicitly exporting credentials.

`rss` reads RSS items and Atom entries with torrent enclosures or explicit magnets. A normal webpage item link alone never becomes a fabricated torrent download. It normalizes dates, enclosure lengths, Torznab category/seeders/peers attributes and Nyaa-style seeders children. The optional definition is `{"query_param":"q"}`; searching adds keywords to that URL parameter, preserving other endpoint parameters. Results are also filtered locally: every whitespace-separated keyword derived from q, season/episode and book/music fields must occur in the title, case-insensitively. Feed-only endpoints may ignore the remote parameter, so local filtering only searches the feed entries returned, not the full tracker history.

`torznab` reads the same feed format and forwards supported Torznab search parameters, including publisher, metadata IDs, category, pagination and search type. It replaces `apikey` with the upstream indexer's stored key, never the caller's management key. Configure the complete upstream API URL, such as an existing Jackett indexer's Torznab URL. Current behavior returns releases, not upstream capability documents.

`unit3d` targets modern UNIT3D's `/api/torrents/filter` relative to the configured base path, or accepts that full endpoint. It sends Bearer authentication, name, IDs, season/episode, perPage (maximum 100), page and descending created_at sorting. It parses `data[].attributes`, percentage freeleech, featured/double-upload factors, IDs and counts. Legacy query-token/boolean-freeleech API profiles are not supported. Standard categories require explicit mapping, for example `{"categories":{"2000":[1],"5000":[2,3]}}`; this mapping works in both request and response directions. Without a mapping, configured indexer categories remain the fallback. With an explicit mapping, unknown or missing tracker category IDs remain uncategorized; they do not inherit the advertised category list. Tracker IDs never become standard Newznab category IDs implicitly. Genres and minimum seed times are not claimed.

`json` requires a small explicit JSON-pointer mapping:

```json
{"items":"/results","title":"/name","download":"/torrent","details":"/details","size":"/bytes","seeders":"/seeders","peers":"/peers","query_param":"q"}
```

`items` points to an array and may be `""` for a root array. Field pointers are relative to each row. `title` and `download` are required strings; details and numeric fields are optional. Sizes are bytes. Relative links resolve against the configured request URL. Unsupported rows are skipped. This is not JSONPath, Go templating or Cardigann.

`html` requires CSS selectors relative to a row:

```json
{"rows":"article.release","title":"h2","download":"a.torrent","details":"a.details","size":".bytes","seeders":".seeders","query_param":"q"}
```

Download/details use `href`; title uses trimmed text. Size/seeders must be plain unsigned decimal values (size in bytes). Selectors and definition keys are validated. No JavaScript, arbitrary headers, regex execution, login forms, implicit scraping of arbitrary detail pages, or Cardigann modifiers are evaluated.

Tests cover malformed/wrong-schema responses, declared and streamed body-size limits, retryable status errors without secret URLs, local RSS filtering, fixture RSS/Atom parsing, rejection of HTML-only/unsafe links, modern UNIT3D normalization and category mapping, encoded Torznab queries with upstream credential replacement, declarative HTML/JSON mapping, URL/unsupported-schema validation and a mock server receiving explicit Bearer/cookie credentials. These are offline compatibility tests, not verification of live authenticated trackers.


## UNIT3D single-file titles

The optional definition key `single_file_release_use_filename` accepts only JSON
`true` or `false` and defaults to false. When enabled, a row with numeric `num_file`
1 (including 1.0), or exact string "1", uses its non-blank string `files[0].name`.
The fields must live alongside `name` in the same attributes object; flat legacy
rows use the same rule. The original `name` is still required. Other counts or
missing, blank and non-string filenames retain the existing name.

This is a conservative subset of the pinned 0DayFiles preference. Upstream
normalizes additional count strings such as "01" or "1 file", and defaults the
checkbox on; Rackett intentionally does neither. This option alone does not
establish native tracker parity or live availability.

For example, a complete definition can contain the option and categories:
`{"single_file_release_use_filename":true,"categories":{"2000":[1],"5000":[2,3]}}`.
These category IDs are illustrative, not a 0DayFiles mapping. PATCH replaces the
whole definition, so resend any existing category mapping when changing the flag.
Custom definitions are not returned by the current public indexer listing.

A binary predating this option rejects it as an unknown definition field during
startup. Before rolling back to such a binary, remove the key with the newer
binary (preserving the rest of the definition), or restore its compatible config
backup. This release does not enable the option in any saved source.

## UNIT3D dotted keywords

The optional `keywords_replace_dots` definition key accepts only JSON `true` or
`false` and defaults to false. When enabled, each ASCII `.` in the assembled search
keywords becomes one ASCII space before encoding the `name` query parameter.
For example, `Some.Show` sends `Some Show`; repeated dots produce repeated spaces.
The existing keyword construction and initial trimming remain unchanged, with no
extra trimming or space collapsing after replacement. Non-ASCII dot lookalikes,
response titles, IDs, the base URL and other request parameters are unchanged.

The pinned 0DayFiles definition applies this filter unconditionally. A native
profile must explicitly set `keywords_replace_dots: true` to reproduce that filter;
this does not establish full query or live tracker parity. It can coexist with
`single_file_release_use_filename` and `categories`. PATCH replaces the whole
definition, and the public indexer listing does not expose it: preserve and resend
the complete definition when changing either option. A binary predating this key
rejects it at startup, even when set to false. Remove it before rolling back to
such a binary, or restore a compatible configuration backup. No saved source is
changed by this release.

## Aggregate source compatibility

Torznab `/all` searches only enabled sources that pass their existing mode and
identifier checks. A source that cannot handle the query is skipped; if none can,
a valid empty feed is returned without upstream requests. An explicit named
source still reports error 203 for an unsupported query. Mode casing is normalized
by the Torznab handler; management API validation is unchanged. Aggregate capabilities advertise the union of source modes and supported parameters;
named-source capabilities describe that source alone. UNIT3D movie and TV modes
advertise the IMDb/TMDb/TVDb parameters already accepted by the adapter. Indexer
listings remain unchanged, and fallback query rewriting is not supported.
