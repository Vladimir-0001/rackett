# Authenticated search shapes and API-key probes

Three engine additions let more logged-in definitions run through the existing cookie,
form and credential-POST sessions, plus one new login family. Each follows the pinned
Jackett behaviour (`CardigannIndexer.cs`, commit `fe8876d2`).

## Form POST searches

Search paths with `method: post` are executable in cookie, form and credential-POST
sessions. The request plan already builds the body exactly as upstream (inputs
order, `$raw`, encoding), and the session sends it as
`application/x-www-form-urlencoded`. Expiry handling is unchanged: a redirect or
missing login marker triggers one relogin and a retry of the same POST. On a
301-303 redirect the body is dropped, as for login POSTs. Non-UTF-8 POST searches
are encoded with the definition charset, as upstream does
([legacy-encodings.md](legacy-encodings.md#search-post-bodies)); the session sends the
plan-encoded body unchanged.

Unlocked: audionews, magico, marinetracker, metaltracker, rintor (fixtures pending).

Harness: form, public and cookie cases set `"search_post": true`. The fixture server
then treats a POST to `search_path` as the search and matches keyword and category
pairs in the body, decoded with the definition charset
(fixture-harness.md).

## JSON and XML search responses in sessions

Cookie and form plans accept `response: {type: json|xml}` on search paths.
Following `CheckIfLoginIsNeeded` (`:951-997`), the login test selector applies only
when the response Content-Type contains `text/html` or is absent. Typed JSON and XML
responses are decoded with the definition charset, checked for challenge markers, and
handed to the plan. Redirects still mean expiry.

Unlocked: dasunerwartete, explosiv-world (form) and bitbazis, ds-reloaded (cookie);
fixtures pending.

## `login: get` API-key probes

`src/builtin_runtime/get_login.rs`. Upstream's `get` login sends one GET to
`path?inputs` (values `WebUtilityHelpers.UrlEncode`, keys raw) without cookies, fails
on HTTP 401 or a matching error selector (the body parsed as HTML), and keeps the
response cookies.

The admitted definitions declare no positive `test`, and every search carries the
key in its own query. Rackett therefore runs the GET as a one-time probe per source
runtime, before its first search, with the same checks. An edit or restart creates a
new runtime and probes again. Difference from upstream: probe response cookies are
not kept. A site that needed them would fail its searches visibly.

Admitted shape: login keys `method`, `path`, `inputs`, `error`; a literal path;
inputs that are literals or a declared text/password setting; no `followredirect` or
`test`. The credential may be a login input or a templated search header (for example
`Authorization: Bearer {{ .Config.apikey }}`). As upstream sends
`Login.Headers ?? Search.Headers` with the login and `Download.Headers ?? Search.Headers`
with downloads, the plan's rendered search headers go with the probe and with this
source's plain downloads. Admission requires the ID in `GET_IDS` plus fixtures.

Candidates (fixtures pending): anthelion-api, binarybay, bitagent, bluetorrents,
czteam-api, fileiplay, hellashut-api, periodical, retrotoon,
shadowbit, stellarwinds, swarmazon-api, ultrabits-api, world-of-tomorrow-api.
Header-key candidates: abnormal-api, alabala-api, desitorrents-api, digitalcore-api,
dirtybytes-api, flood, g3minitr4ck3r-api, generationfree-api, hdolimpo-api,
polishtorrent, postman-api, teamflix, theoldschool-api, torrentclaw, trackerzero.
Templated login paths (scenetime-api, v3x, yggreborn-api) stay blocked.

Harness (public): `"settings": {"apikey": "fixture-source-key"}` (anything but
`fixture-key`, which is Rackett's own Torznab key in the harness) and
`"probe": "/torznab/api?t=caps&apikey=fixture-source-key"` (the exact probe
path and query). The probe is answered with an empty capabilities document and not
counted among search requests. Presented results must not contain any setting value.
Header-key cases add `"expected_headers": {"authorization": "Bearer fixture-source-key"}`
(lower-case names). Every request must carry them; no other Authorization is allowed.

JSON sources set `"empty_body"` to their literal no-results response (a JSON body cannot
carry the `<!-- rows:start -->` markers), and Torznab XML sources need the RSS `channel` as
their `empty_html_container` in `src/builtin.rs` so an HTML error page is not mistaken for
an empty result. Definitions that fix `downloadvolumefactor` (e.g. `text: 1`) set
`"download_factor": "1"`.
