# Legacy single-byte encodings

42 of the 583 pinned Jackett definitions declare a non-UTF-8 `encoding`. Rackett
supports every label they use, with tables taken from the data .NET 9 itself uses.
Admission still depends on each definition's other features.

| Label (case-insensitive) | Definitions | Table source |
| --- | --- | --- |
| `iso-8859-1` | 7 | Identity mapping (`Latin1Encoding`) |
| `windows-1251` | 17 | Generated |
| `ISO-8859-2` | 9 | Generated |
| `windows-1252` | 3 | Generated |
| `windows-1250` | 2 | Generated |
| `windows-1256` | 1 | Generated |
| `windows-1255` | 1 (fuzer) | Generated |
| `windows-874` | 1 (unlimitz) | Generated |
| `tis-620` | 1 (siambit) | Code page 874, as in .NET |

Aliases such as `latin1`, `cp1251` or `iso-8859-11` are still rejected; no pinned
definition uses them.

## .NET mapping evidence (2026-11-01)

Jackett targets `net9.0` and calls `Encoding.GetEncoding(Definition.Encoding)`
(`CardigannIndexer.cs:126`). For the code pages that resolves through
`CodePagesEncodingProvider`, whose tables are the binary `codepages.nlp` in
`dotnet/runtime` at `v9.0.0`
(`src/libraries/System.Text.Encoding.CodePages/src/Data/codepages.nlp`, SHA-256
`0cad998c…33e9fb`). `tools/gen_single_byte_tables.py` now reads that file directly,
following `BaseCodePageEncoding.cs` (file, index and code-page headers) and
`SBCSCodePageEncoding.cs` (`LoadManagedCodePage`, `ReadBestFitTable`):

- **Names.** `EncodingTable.Data.cs` maps `windows-1255` to 1255 and both
  `windows-874` and `tis-620` (and `dos-874`, `iso-8859-11`) to 874.
- **Decoding.** Every SBCS page in the file has 256 distinct non-`U+FFFD`
  entries and an empty bytes-to-Unicode best-fit list, so `GetString` is total and
  never produces `?`. The bytes the Unicode vendor tables
  (`MAPPINGS/VENDORS/MICSFT/WINDOWS/CP1255.TXT`, `CP874.TXT`) leave undefined decode as:
  - windows-1255: C1 controls for `0x81 0x8A 0x8C-0x90 0x9A 0x9C-0x9F`, `0xCA` to
    U+05BA, `0xD9-0xDF` to U+F88D-U+F893, `0xFB 0xFC` to U+F894-U+F895 and `0xFF` to
    U+F896.
  - windows-874: C1 controls for `0x81-0x84 0x86-0x90 0x98-0x9F`, `0xDB-0xDE` to
    U+F8C1-U+F8C4 and `0xFC-0xFF` to U+F8C5-U+F8C8.
- **Cross-checks.** All 256 entries of both pages equal the `MBTABLE` of Microsoft's
  `WindowsBestFit/bestfit1255.txt` and `bestfit874.txt` on unicode.org. Their
  Unicode-to-bytes best-fit lists (96 and 138 entries) equal those files' `WCTABLE`
  entries beyond the exact mappings. Every byte that CPython's `cp1255`/`cp874` codecs
  (the vendor tables) define agrees; the generator asserts this. The five pages
  admitted earlier regenerate byte for byte from the `.nlp` file, which confirms their
  CPython-derived tables.
- **Encoding.** `BaseCodePageEncoding` installs `InternalEncoderBestFitFallback`:
  a character without an exact byte becomes its best-fit byte, else `?`; a surrogate
  pair becomes `??` (`EncoderBestFitFallback.cs:81-115`). `Latin1Encoding` uses
  `EncoderLatin1BestFitFallback` (`Latin1Encoding.cs:29-33`, data in
  `EncoderLatin1BestFitFallback.Data.cs`). Rackett keeps rejecting such text in queries,
  credentials and request bodies, and uses best fit only for `urldecode` (below).

Regenerate with the two `v9.0.0` source files:

```sh
python3 -I tools/gen_single_byte_tables.py codepages.nlp \
    EncoderLatin1BestFitFallback.Data.cs > src/cardigann_exec/single_byte.rs
```

## Behaviour (pinned Jackett `fe8876d2`)

- **Responses.** `WebResult.Encoding` prefers the request encoding, which the
  definition sets, over the response's `charset` header. Rackett decodes search
  pages, cookie-session test pages, login forms and login responses with the
  definition charset and ignores the header charset, BOMs and `<meta>` tags. Every
  byte maps to exactly one character, so decoding cannot fail.
- **Query strings.** `WebUtilityHelpers.UrlEncode(value, encoding)` encodes the
  text in the code page, then percent-encodes it with upper-case hex.
- **Credential POST and form bodies.**
  `WebClient.FormUrlEncodedContentWithEncoding` uses
  `HttpUtility.UrlEncode(text, encoding)` for each key and value, which produces
  lower-case hex, and sends `application/x-www-form-urlencoded`. UTF-8 definitions
  keep the existing `FormUrlEncodedContent` path.
- **Search POST bodies** use the same encoder (see below).
- **Unrepresentable text** in a query, credential or search body, such as Cyrillic
  sent to a windows-1250 site, is an error. .NET would substitute a best-fit or `?` byte;
  Rackett refuses to guess.
- **FlareSolverr** returns already-decoded browser text, so challenge solving stays
  limited to UTF-8 definitions. The `urlencode` filter stays rejected for non-UTF-8
  definitions; no pinned one uses it.

## `urldecode`

Upstream's filter is `WebUtilityHelpers.UrlDecode(Data, Encoding)`
(`CardigannIndexer.cs:1198-1199`), with the definition encoding:
`encoding.GetString(WebUtility.UrlDecodeToBytes(encoding.GetBytes(text)))`
(`WebUtilityHelpers.cs`). `Charset::url_decode` does the same:

1. `GetBytes` with the encoding's default best-fit fallback. Page text decoded with
   the same charset always round-trips; other characters (from entities, for example)
   take their best-fit byte, `?`, or `??` for a non-BMP character.
2. `WebUtility.UrlDecodeInternal` (`WebUtility.cs:510-550`): `+` becomes a space, and
   `%` followed by two hex digits (either case) becomes that byte; any other `%` is
   literal.
3. `GetString`, which is total for single-byte pages. UTF-8 replaces invalid
   sequences with U+FFFD, as before.

AfterShock (ISO-8859-1) runs `querystring name` and then `urldecode` on its download
links, so a doubly escaped `%25C9t%25E9` title becomes `Été`.

Known divergence, not in this module: upstream `querystring` uses
`QueryHelpers.ParseQuery` from Microsoft.AspNetCore.WebUtilities 2.3.13, which calls
`Uri.UnescapeDataString(value.Replace('+', ' '))` (`QueryHelpers.cs:165-166`, aspnetcore
`release/2.3`). `UnescapeDataString` is documented to leave escapes that are not valid
UTF-8 (such as a single `%E9`) as text. Rackett's `querystring` decodes them to U+FFFD.
The AfterShock fixture therefore uses `%25`-escaped bytes; a link with a bare `%E9`
would title `?` here and `é` upstream. Fixing it means changing `querystring` in
`filters.rs`.

## Search POST bodies

`method: post` search paths of non-UTF-8 definitions are supported since 2026-11-01.
Upstream builds every search through `RequestWithCookiesAsync(..., data:
queryCollection)` (`CardigannIndexer.cs:1605-1606`), which sets the request encoding
to the definition encoding (`BaseIndexer.cs:598-612`). `HttpWebClient2` turns POST
data into `FormUrlEncodedContentWithEncoding(PostData, Encoding)`
(`HttpWebClient2.cs:163-166`), the same encoder as credential bodies
(`WebClient.cs:242-263`). The request plan therefore:

- encodes every key and value with `HttpUtility.UrlEncode(text, encoding)`:
  code-page bytes, lower-case hex, `+` for a space, `-_.!*()` literal and `~`
  escaped. Keys are encoded too (`f[]` becomes `f%5b%5d`), and repeated keys keep
  their order;
- re-encodes `$raw` values, which `applyGoTemplateText` has already UTF-8 encoded with
  `WebUtility.UrlEncode` (`:1565`), so their `%` and `+` are encoded again, as in
  UTF-8 POST bodies and GET queries;
- encodes literal values such as file-tracker's `submit: "%A0%A0Search%A0%A0"` as text
  (`%25A0%25A0Search%25A0%25A0`);
- rejects unrepresentable text with an error instead of a `?` byte.

UTF-8 definitions keep `FormUrlEncodedContent`. Unit tests in
`src/cardigann_exec/request.rs` check the bodies byte for byte
(`non_utf8_post_bodies_match_form_url_encoded_content_with_encoding`).

Authenticated sessions hand the search loop text they have already decoded.
`Fetched::decoded` marks such bodies so they are never decoded a second time; the
first end-to-end fixture caught that bug as Latin-1 mojibake.

## Tables

`src/cardigann_exec/single_byte.rs` is generated by
`tools/gen_single_byte_tables.py` from CPython's code-page codecs, which follow the
Unicode Consortium vendor mappings. Bytes those mappings leave undefined (for
example `0x98` in windows-1251, or `0x81`, `0x8D`, `0x8F`, `0x90` and `0x9D` in
windows-1252) decode to the C1 control with the same value, as Windows and .NET's
`CodePagesEncodingProvider` do. Unit tests check that all 256 bytes round-trip,
spot-check published characters, and check URL and form encoding.

## Fixture harness

The form, cookie and public lifecycle harnesses compile each case's plan to find its
charset. For a non-UTF-8 definition they:

- serve every page as bytes in that code page, with a misleading
  `charset=utf-8` header (or none, in the public harness);
- require a native, non-ASCII expected title;
- for form and credential-POST logins, use a username native to the code page,
  check that the posted body uses lower-case `HttpUtility` escapes, and decode it
  with the definition charset before comparing fields;
- for `search_post` cases (all three harnesses), apply the same check and decoding
  to the search body before matching keyword and category pairs.

## Admitted non-UTF-8 runtimes

| Code page | IDs |
| --- | --- |
| ISO-8859-1 | deildu, tribalmixes |
| windows-1251 | kinorun, peersfm, traht, file-tracker (POST search), bigfangroup (public), megapeer (public), noname-club (public, POST search) |
| ISO-8859-2 | bestcore, libranet, party-tracker, redstartorrent, whiteangel |
| windows-1252 | xwtorrents |
| windows-1250 | trezzor, trezzorcookie (cookie) |
| ISO-8859-1 (`urldecode`) | aftershock (cookie) |
| windows-1255 | fuzer (cookie) |
| windows-874 / TIS-620 | unlimitz (form, image CAPTCHA absent), siambit (credential POST, `download.before` GET) |

The Thai row selectors use unquoted numeric attribute values (`[border=1]`), which
AngleSharp 1.8.6 accepts as number tokens (`CssSelectorConstructor.cs:309-317`).
`css.rs` (`rewrite_numeric_values`) quotes digit-only values before the selectors crate
parses them. Signs, decimals, exponents and dimensions stay rejected.

DeildU was the first: its title `Fixture Íslensk Útgáfa` and Latin-1 username pass
the full lifecycle.

## Remaining candidates

- **noname-clubl** (cookie, windows-1251 POST search): its search passes the offline
  cookie layout test, but the cookie session parses `login.test.selector` with
  `scraper`, which has no `:contains`, and its test is
  `table.menutable a.mainmenu:contains("Выход")`. Parked in
  `tests/fixtures/pending/noname-clubl-retry.json`.

- **Cookie retries:** bithumen (leechers field parse), newheaven (four paginated
  requests with a 2 s delay), xwtorrents2fa (category expectation).
- **Retries:** karagarga (needed `normal_upload` 1.1, now supported) and 0daykiev
  (its default `stripcyrillic` removes Cyrillic titles; use non-Cyrillic
  windows-1251 text).

The remaining non-UTF-8 definitions have other blockers: .NET regex features,
`followredirect`, CAPTCHA or download resolution. No encoding blocker is left.

Fixtures: `tests/fixtures/cookie-batches/batch8.json` (aftershock, fuzer) and
`tests/fixtures/form-batches/batch26.json` (siambit, unlimitz). Pages are served in the
code page, with a misleading `charset=utf-8` header. Logins use `fixture-משתמש` and
`fixture-ผู้ใช้`.
