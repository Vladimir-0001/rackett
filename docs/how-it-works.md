# How Rackett works

This guide is for people who want to read or change the code. For using Rackett,
see the [README](../README.md).

## The big picture

```text
Sonarr / Radarr ──Torznab──▶ protocol.rs ──▶ engine.rs ──▶ one runtime per source
                                               │  cache        ├─ custom adapters (adapters.rs)
Web interface ──JSON API──▶ api.rs ────────────┘               └─ Cardigann sites (builtin*.rs)
```

1. A search arrives as a Torznab request (from an app) or a JSON request (from the
   web interface).
2. The **engine** decides which sources can answer it, serves anything still in the
   cache, and sends the rest to the sources in parallel, within per-source time and
   rate limits.
3. Each source **runtime** turns the query into HTTP requests, reads the response
   and returns normalized releases (title, size, seeders, download link and so on).
4. Results are merged, cached and returned as a Torznab feed or JSON.

## Code layout

| Path | Purpose |
| --- | --- |
| `src/main.rs` | Command-line options and startup |
| `src/protocol.rs` | Torznab endpoints, feed output, download links |
| `src/api.rs` | JSON API used by the web interface |
| `src/engine.rs` | Search scheduling, deadlines, caching, activity |
| `src/store.rs` | Saving configuration to disk safely |
| `src/adapters.rs` | Custom sources: RSS, Torznab, UNIT3D, JSON, HTML |
| `src/cardigann.rs` | Parser and checker for Jackett's YAML definitions |
| `src/cardigann_exec/` | Runs definitions: templates, filters, CSS selectors, dates, request building |
| `src/builtin.rs` | The bundled catalog and which sites are ready |
| `src/builtin_runtime/` | HTTP transport, logins (cookie, form, API), FlareSolverr, sessions |
| `src/torrent.rs` | Checks downloaded `.torrent` files |
| `web/` | The web interface (plain JavaScript, embedded in the binary) |
| `vendor/jackett/` | Jackett's site definitions, unchanged, with checksums |
| `tests/` | Integration tests and site fixtures |

## Cardigann definitions

Jackett describes most sites with a YAML file: the site's categories, its login
method, how to build a search URL, and which parts of the result page hold the title,
size, seeders and download link. Rackett embeds all of these files from a fixed
Jackett version (`vendor/jackett`, commit `fe8876d2`) and runs them with its own
engine.

The definitions rely on .NET and Go behaviour: Go-style templates, .NET regular
expressions, .NET date formats, AngleSharp CSS selectors, and text encodings.
Rackett reimplements that behaviour in Rust. Where it can't match Jackett exactly, it
refuses the definition rather than guessing.

To see which definitions compile and what blocks the rest:

```sh
cargo run --locked -- definitions check vendor/jackett/definitions --json
cargo run --locked --quiet --example cardigann_diagnostics
```

## How a site becomes ready

A definition that compiles is not automatically trusted. A site is marked ready only
when all of the following hold:

1. Its ID is listed in `src/builtin.rs`.
2. A test case records the definition's checksum and the exact behaviour taken from
   it: login fields, search parameters, categories and dates.
3. A synthetic copy of the site's pages, kept under `tests/fixtures/`, passes a full
   lifecycle test against a local fake server. That covers login, search, empty
   results, caching (the second search makes no network request), authenticated
   download, session renewal and expiry, settings edits, restart, and error pages
   that must not leak credentials.

Sites are grouped by how they authenticate: public, supplied cookie, login form,
credential POST, or API key. Each group has a shared test harness, such as
`tests/form_layout_runtime.rs` or `tests/cookie_layout_runtime.rs`. Adding a site to
an existing group usually means adding a fixture page and a test case, with no Rust
changes.

These tests prove Rackett behaves as the definition describes. They don't prove the
real site is online or that an account works.

## Safety rules the code follows

- Rackett listens only on `127.0.0.1`. Remote access goes through a reverse proxy
  you configure.
- Settings changes require a matching `Origin` header, which blocks cross-site
  requests from other web pages.
- Credentials are never written to logs, errors or Torznab output. Download links
  are short-lived opaque tickets.
- Every request has a deadline and a size limit. Redirects to other sites are
  refused.
- Only temporary failures of plain GET requests are retried, within the same deadline.

## Running the checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
node --check web/app.js
```

The full test suite takes several minutes because the lifecycle tests respect each
site's real request delays.
