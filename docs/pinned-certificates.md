# Pinned site certificates (`certificates:`)

15 pinned definitions declare `certificates:`, a list of SHA-1 hashes for a site
certificate that is self-signed or expired. Before this change every one of them
was blocked as `$.certificates: transport certificate policy`. Rackett now honours
the list for the source's own site host, and nothing more.

## Upstream behaviour (pinned Jackett)

- `CardigannIndexer.cs:245`: each hash is added as trusted for
  `new Uri(SiteLink).Host`, the configured site host only.
- `WebClient.cs:103` `AddTrustedCertificate` uppercases the hash.
- `HttpWebClient2.cs:44` `ValidateCertificate`: `certificate.GetCertHashString()`
  (uppercase hex SHA-1 of the server's leaf certificate DER) is looked up. If the
  hash is trusted for `request.RequestUri.Host` the connection is accepted
  regardless of policy errors. Otherwise it is accepted only with no policy errors.

## Rackett behaviour

`src/pinned_tls.rs` implements a rustls `ServerCertVerifier`:

1. Normal WebPKI verification runs first, against the same `webpki-roots` store and
   crypto provider (the process default, otherwise `ring`) that reqwest's
   `rustls-tls` backend uses. A site whose certificate has since become valid works
   as it always would.
2. Only if that fails is the leaf accepted, and only when both hold:
   - the SHA-1 of the `end_entity` DER is in the definition's list (hex compare is
     case-insensitive: hashes are parsed to bytes);
   - the TLS server name equals the configured site host (`Indexer.url`).
3. TLS 1.2 and 1.3 handshake signatures are **always** verified with the provider's
   algorithms. A pin trusts one certificate, not the connection. A server that
   presents the pinned certificate without its private key is still rejected.

The verifier is installed with `reqwest::ClientBuilder::use_preconfigured_tls` on
the engine's per-source runtime client (`engine.rs` `runtime_with`), which serves
public searches, detail pages and downloads. The client reads the pins from the
bundled definition through `builtin::certificates`. The cookie/form/credential-POST
session client (`auth.rs` `CookieSession`), which serves login and authenticated
requests, applies the same pins through `pinned_tls::apply`.

The config mirrors reqwest's defaults: TLS 1.2 and 1.3, SNI on, and HTTP/1.1 ALPN.
The crate doesn't enable reqwest's `http2` feature, so reqwest itself offers only
HTTP/1.1. Sources without `certificates` are not touched: `pinned_tls::apply`
returns their builder unchanged.

### Accepted definition shape

`certificates` must be a sequence of 1 to 4 strings, each exactly 40 hex digits.
Every pinned definition has exactly one. Non-string entries already fail
definition validation. Any other shape (empty list, scalar, wrong length, non-hex,
more than four) compiles to no pins (`pinned_tls::Pins::compile` fails). The
plan keeps its `$.certificates` blocker in all cases. The catalog gate (`builtin.rs`
`eligible`) admits that blocker only when the pins compile **and the definition has no
`login` block**. Until session clients install pins, a login source with pins stays
blocked. Fixture tests use plain HTTP, so admitting such a source would pass its
fixtures and then fail TLS against the live site.

## Security reasoning

- **Narrower than upstream.** Upstream's trust table is process-wide, keyed by
  hash to host. Here each pin lives on one source's clients, so another source that
  requests the same host gets no trust from it.
- **Redirects and links cannot carry trust.** The host check compares the actual TLS
  server name. The engine client already rejects cross-origin redirects, and session
  clients don't follow redirects at all. If a request did reach another host, it
  would get no pinned trust, because the server name would differ.
- **No blanket acceptance.** There is no `dangerous` accept-all path. Signature
  checks are never skipped, and WebPKI errors for unpinned leaves are returned
  unchanged.
- **SHA-1 is upstream's identifier.** A SHA-1 second-preimage attack on a specific
  certificate is not practical. Supporting SHA-256 would be stronger, but the
  definitions don't supply it.
- **What a pin does not check.** Expiry, name constraints, the chain and revocation
  are ignored for a pinned leaf, as upstream ignores every policy error. The
  definition's hash is the trust anchor.

## Catalog effect

All 12 definitions blocked by this feature now compile with valid pins:

| Status | Definitions |
|---|---|
| Ready (fixture-tested, 2026-10-30) | torrentby (public); krazyzone, leech24, thegeeks, theplace, theshow, torrent-turk (cookie); borgzelle, peeratiko, ptfiles, trackermk (form/credential POST) |
| Also blocked by another feature | extremebits (`login.selectors: true` is unsupported) |

The other three definitions that pin certificates are blocked elsewhere:
crazyspirits-api and ebookbay by unsupported .NET regexes, and vault-network by its
login and download shapes.

## Current state

- Done: verifier, definition validation, engine-client and session-client hooks, and
  the catalog gate. All eleven sources above are admitted with fixture lifecycles,
  which run over plain HTTP (the pins are exercised by the unit tests).

## Not supported

- Jackett's global `IgnoreSslErrors` setting.
- Pins on hosts other than the configured site host, such as alternate `links`
  mirrors reached via a changed site URL. Those are trusted only if the operator
  configures that host as the site URL, which matches upstream.
- Pinning intermediate or root certificates, or SHA-256 pins (upstream has neither).
- The FlareSolverr path: the solver fetches the page itself, so pins don't apply
  to that fetch.

## Validation

- `cargo test --locked --test pinned_certificates` generates a self-signed leaf with
  `rcgen` at test time. It serves the leaf over loopback with `tokio-rustls`, under
  synthetic host names resolved through `ClientBuilder::resolve`, and shows:
  - the pinned hash (any case, alone or in a list) for the configured host is
    accepted;
  - a wrong hash is rejected;
  - the right hash on a different SNI host (the leaf is valid for both names) is
    rejected;
  - a client without pins (any normal source) rejects the certificate;
  - the pinned certificate presented with a different private key is rejected,
    because the handshake signature fails;
  - definition-shape validation on the pinned torrentby definition;
  - none of the 12 catalog entries reports a certificate blocker as its first
    blocker, and torrentby reaches the fixture-admission step.
- `src/pinned_tls.rs` unit tests cover the YAML shapes and site URL validation.
- `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`
  and `cargo test --locked --lib` pass.

## Dependencies

- `rustls = "=0.23.46"` (ring, std, tls12) and `webpki-roots = "=1.0.11"`, pinned to
  the versions reqwest already locks. `use_preconfigured_tls` requires the identical
  `rustls::ClientConfig` type, so these pins must move together with reqwest.
- Dev-only: `rcgen 0.13.2` (with `yasna 0.5.2`, the only newly downloaded crates) and
  `tokio-rustls = "=0.26.6"` (already locked).
