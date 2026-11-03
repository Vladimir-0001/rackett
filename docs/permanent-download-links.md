# Permanent download links

Sonarr and Radarr store the download links they get from a search and may use them much
later, for example under a delay profile, or after a restart of either program. Rackett's
earlier links were in-memory tickets that lasted two hours and were lost on any restart or
source edit, so delayed grabs failed (see review-2026-11-02.md).
Links are now permanent, as Jackett's `/dl/` links are.

## How a link works

A `.torrent` link (or a detail page that resolves to a magnet, as for 1337x) is presented
as `<base>/dl/<token>?apikey=<key>`. The token is `src/ticket.rs`'s sealed record of:

- the source ID;
- a fingerprint of the source's credential-bearing configuration;
- the release link;
- the release title, used as the download file name and capped at 200 characters.

It is sealed with ChaCha20-Poly1305 (`ring`) under a 32-byte server key in `ticket.key`,
beside `config.json`. The key file is created on first start with mode 0600. Nothing is
stored per link, so there is no table to lose on restart and no time limit. Release links
often contain a tracker passkey. Sealing them, rather than only signing them, keeps the
passkey out of the clients and databases that store these URLs.

Magnet links found in search results are still returned as they are.

## When a link stops working

On use, Rackett opens the token and requires that:

- the source still exists and is enabled;
- the fingerprint of the source's current configuration matches the one in the token;
- the release link is still on the source's origin, along with the existing detail-page
  and download checks.

The fingerprint is SHA-256 over the source kind, URL, API key, cookie and definition
(including its settings). So:

| Change | Existing links |
| --- | --- |
| Restart | keep working (same `ticket.key`) |
| Rename, tags, categories, request delay, retries | keep working |
| Disable | stop until the source is enabled again |
| URL, API key, cookie, password or any definition setting | retired |
| Delete | retired |
| Search results produced by a runtime that was replaced before they were presented | retired |
| Rackett API key rotated | rejected (the `apikey` parameter no longer matches) |
| `ticket.key` deleted or replaced | every link becomes "Unknown download link" |

This differs from Jackett in one respect: Jackett keeps old links after a credential
change. Rackett retires them. Release links can embed the credential (Milkie's `key=`),
so an old link would otherwise keep sending a rotated or revoked credential.

Errors are `Download link retired: the source was removed, disabled or changed; search
again` for a retired link, and `Unknown download link` for a token this server did not
seal.

## Operations

- Back up `ticket.key` with `config.json`. Restoring the configuration without the key
  invalidates every link already handed out.
- To revoke every outstanding link at once, stop Rackett and delete `ticket.key`; a new
  key is created on the next start.
- Links longer than 8 KB after sealing are not issued; the release then has no download URL.

## Tests

- `src/ticket.rs`: sealing round-trips; foreign, tampered and oversized tokens are
  rejected; the key file is private and stable; and the fingerprint scope.
- `src/protocol.rs`: links are sealed and hide the passkey, stale runtimes get a dead
  fingerprint, and magnets pass through.
- `src/engine.rs`, `links_survive_restarts_and_harmless_edits_but_not_credential_changes`.
- `tests/download_ticket_lifecycle.rs`: renames, delay edits and restarts keep links; URL
  and key changes, deletion and edits that race a search retire them.
