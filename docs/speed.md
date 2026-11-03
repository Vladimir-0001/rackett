# Speed against Jackett (2026-11-02)

The goal was cached searches and result handling 5–10× faster than Jackett on every
metric. This records the measurement method, the starting point, what changed and
the final numbers, including the metrics that still fall short and why.

**Result.** Every cached-search, concurrency, startup and idle-memory metric is now
5–73× ahead of Jackett. Uncached searches by one client are 2.4–4.9× faster and use
3.8–5.6× less CPU. Before this work Rackett tied or trailed Jackett on half of these
metrics.

## How it is measured

[`tools/benchmark_speed.py`](../tools/benchmark_speed.py) runs both engines against the
same synthetic upstream on loopback:

- **Rackett:** the release binary under test, native.
- **Jackett:** the already-installed LinuxServer image `v0.24.2934-ls18`
  (`sha256:f31a5727…`), a disposable container with host networking. Its bundled
  `1337x.yml` and `acgrip.yml` are checked byte for byte against the pinned definitions.
- **Upstream:** `examples/loadgen.rs serve`, which returns 1337x and ACG.RIP listing
  pages. A search term starting `rN` returns N rows.
- **Load:** `examples/loadgen.rs`, a closed-loop HTTP client on keep-alive connections.
  Each response is checked for the exact item count, outside the timed interval.

| Scenario | What it measures |
| --- | --- |
| caps | `t=caps` on one source |
| warm 1337x 20 rows | A cached search: lookup, link sealing, XML |
| warm ACG.RIP 100 rows | The same with 100 `.torrent` links, each a sealed `/dl/` token |
| warm aggregate | `all` across both sources: merge, dedupe, sort |
| cold ACG.RIP N rows | A new query every request: fetch, HTML parse, extraction, XML |

Concurrent scenarios use 64 (warm) or 16 (cold) clients.

**Timing on this host needs care.** The CPU governor is `powersave` (intel_pstate), and
sampled cores dropped from 4.6 GHz to 800 MHz within seconds. Back-to-back runs of one
engine alone therefore swing 2–5× between runs, for either engine. Two measures stay
fair:

- **Paired latency (primary).** Single-client scenarios alternate engines request by
  request, so both see the same CPU state. Each request then reaches an idle process,
  as sparse real-world Torznab queries do. That adds a fixed wake-up cost of about
  0.1 ms per request.
- **CPU time per request**, read from `/proc` for each engine's process.

A `--sequential` mode gives back-to-back numbers too.

Each scenario runs three times per engine and reports the median. No real trackers,
accounts or downloads are involved.

```sh
cargo build --release --locked --bin rackett --example loadgen
TMPDIR=/dev/shm python3 tools/benchmark_speed.py \
  --jackett-image sha256:<installed-jackett-image-id> \
  --rackett-binary target/release/rackett --scale 0.5 \
  --output /tmp/speed.json            # add --sequential for back-to-back timing
cargo run --release --example extract_bench -- 100 300   # extraction alone, µs/row
```

Raw reports are in [benchmarks/2026-11-02](benchmarks/2026-11-02/):
`speed-baseline.json` (before), `speed-paired.json` and `speed-sequential.json` (after).
Measured on an i7-9700K (8 cores) shared with normal desktop services, release binary
SHA-256 `f2f5088b7ef2150b99d8e6bb69e451b8e8684f624b162619e124229e25851d36`.

## Before

Measured back to back on main `5fd27df`, before any change. Each cell is the median p50
of two runs.

| Scenario | Rackett | Jackett | Speedup |
| --- | ---: | ---: | ---: |
| caps | 0.065 ms | 0.718 ms | 11.0× |
| warm 1337x 20 rows, 1 client | 0.455 ms | 1.236 ms | 2.7× |
| warm 1337x 20 rows, 64 clients | 3,440 req/s | 3,356 req/s | 1.0× |
| warm ACG.RIP 100 rows, 1 client | 1.610 ms | 3.547 ms | 2.2× |
| warm ACG.RIP 100 rows, 64 clients | 1,394 req/s | 673 req/s | 2.1× |
| warm aggregate, 1 client | 1.526 ms | 1.615 ms | 1.1× |
| cold ACG.RIP 20 rows, 1 client | 2.312 ms | 2.020 ms | **0.9×** |
| cold ACG.RIP 20 rows, 16 clients | 417 req/s | 940 req/s | **0.4×** |
| cold ACG.RIP 500 rows, 1 client | 30.4 ms | 45.8 ms | 1.5× |
| startup to first answered search | 0.81 s | 3.46 s | 4.3× |
| memory after the run | 119 MiB | 237 MiB | 2.0× |

## After

### Paired latency and CPU per request

| Scenario | Rackett p50 | Jackett p50 | p50 | p99 | Throughput | CPU per request (Rackett / Jackett) | CPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| caps | 0.166 ms | 0.779 ms | 4.7× | 5.7× | 5.0× | 47 / 1,065 µs | 22.6× |
| warm 1337x 20 rows, 1 client | 0.165 ms | 0.976 ms | 5.9× | 6.4× | 6.1× | 61 / 1,028 µs | 16.8× |
| warm 1337x 20 rows, 64 clients | 1.190 ms | 19.419 ms | 16.3× | 12.2× | 15.4× | 44 / 1,208 µs | 27.7× |
| warm ACG.RIP 100 rows, 1 client | 0.311 ms | 3.521 ms | 11.3× | 14.4× | 12.4× | 133 / 4,158 µs | 31.2× |
| warm ACG.RIP 100 rows, 64 clients | 2.783 ms | 103.258 ms | 37.1× | 31.6× | 35.4× | 77 / 5,655 µs | 73.4× |
| warm aggregate, 1 client | 0.232 ms | 1.696 ms | 7.3× | 12.2× | 7.9× | 108 / 1,942 µs | 17.9× |
| cold ACG.RIP 1 row, 1 client | 0.420 ms | 1.017 ms | 2.4× | 3.3× | 2.5× | 255 / 1,164 µs | 4.6× |
| cold ACG.RIP 20 rows, 1 client | 0.827 ms | 2.254 ms | 2.7× | 4.6× | 2.9× | 655 / 2,491 µs | 3.8× |
| cold ACG.RIP 100 rows, 1 client | 2.420 ms | 9.039 ms | 3.7× | 4.6× | 3.7× | 2,296 / 9,519 µs | 4.1× |
| cold ACG.RIP 20 rows, 16 clients | 1.680 ms | 17.568 ms | 10.5× | 7.7× | 9.8× | 562 / 2,989 µs | 5.3× |
| cold ACG.RIP 500 rows, 1 client | 9.572 ms | 46.559 ms | 4.9× | 6.1× | 5.0× | 9,091 / 51,364 µs | 5.6× |

### Back-to-back (sequential) latency, where clock scaling is not the main effect

| Scenario | Rackett p50 | Jackett p50 | Speedup |
| --- | ---: | ---: | ---: |
| caps | 0.059 ms | 0.728 ms | 12.3× |
| warm 1337x 20 rows, 1 client | 0.096 ms | 0.921 ms | 9.6× |
| warm ACG.RIP 100 rows, 1 client | 0.145 ms | 3.470 ms | 23.9× |
| warm aggregate, 1 client | 0.086 ms | 1.602 ms | 18.6× |
| cold ACG.RIP 500 rows, 1 client | 9.351 ms | 45.943 ms | 4.9× |

Sequential cold runs of 1–100 rows are not shown: they swung with the CPU clock. A
repeat of the 20-row run measured Rackett at 0.70–0.72 ms and Jackett at 3.1–5.1 ms.
The JSON keeps every run.

### Process

| Metric | Rackett | Jackett | Ratio |
| --- | ---: | ---: | ---: |
| Startup to first answered caps | 0.02–0.04 s | 3.4–7.8 s on a quiet host | ≥ 80× |
| Idle memory (RSS) | 20 MiB | 155 MiB | 7.8× |
| Memory after the full run (RSS) | 48–50 MiB | 236–252 MiB | 4.7–5.3× |
| Peak memory (high-water mark) | 51–57 MiB | 272–279 MiB | 4.9–5.3× |

## What changed

Each change keeps the existing behaviour and tests unless it says otherwise.

- **Shared cached results** (`src/engine.rs`). A cache entry is an immutable `ResultSet`
  behind an `Arc`. Hits share it instead of deep-copying every release three or four
  times per request. Torznab assembles its feed from views into the sets.
- **Rendered items are kept.** From a result set's second showing, each release keeps
  its rendered Torznab item, keyed exactly by presentation context: an API-key and
  base-URL digest plus the source's credential fingerprint. A key rotation, base-URL
  change or credential edit therefore renders afresh, and a query asked only once
  stores nothing.
- **Cheaper link sealing** (`src/ticket.rs`, `src/protocol.rs`).
  - Each runtime computes its credential fingerprint once. Before, it was recomputed
    from serialized JSON for every release.
  - Seal nonces come from the thread CSPRNG instead of one `getrandom` syscall per
    link.
  - XML escaping is one table-driven pass. Before, each field made six allocations.
  - Dates are formatted directly; a test checks the output against chrono's
    `to_rfc2822` over 33,000 dates, offsets and a leap second.
- **Startup** (`src/builtin.rs`). Category lookup no longer builds the whole
  583-definition catalog, which took 0.74 s on the first search, and then scanned it
  linearly on every aggregate search.
- **Timer rounding** (`src/engine.rs` `wait_until`). Tokio rounds even an already
  elapsed sleep up to its next 1 ms tick. Rate-gate waits whose turn has already come
  now return at once, which removed about 1 ms from every uncached search of a
  zero-delay source.
- **Extraction** (`src/cardigann_exec`):
  - Parsed CSS selectors and compiled templates are cached per thread. They used to be
    parsed again for every field of every row.
  - Literal templates skip compilation and the context walk.
  - Rows reuse one template context, and skip recording `.Result.*` values when the
    definition never reads them.
  - Each row's node is found once, and parsed base URLs are reused.
  - A field selection stops at its first match, as AngleSharp's `QuerySelector` does.
    *Behaviour change:* a selection that finds a match before exhausting the 32 MiB
    `:contains` scan budget now returns it instead of failing. Selectors with no match
    still fail on the budget; see `tests/cardigann_css_contains.rs`.
- **Uncached concurrency.** A source with no request delay and no session state (no
  login, API session, saved cookie, solver or GET probe) now fetches distinct queries
  side by side. Identical queries still share one fetch. Its searches hold the rate
  gate shared, so downloads still queue behind them and revalidate. Every other source
  keeps one fetch at a time, as before.
- **Inline parsing.** Responses up to 32 KiB are parsed on the search task. Larger ones
  still go to the blocking pool.
- **Cache budget** (*behaviour change*). Each source keeps at most 64 queries *and* 5,000
  releases, evicting the oldest first. Before, the only cap was 64 queries, which could
  hold 64,000 releases. Jackett's default is 1,000 results per indexer.
- **Allocator and sockets.**
  - mimalloc is the global allocator, built with `no_thp`; without that option it
    raised idle RSS from 15 to 57 MiB.
  - Accepted sockets set `TCP_NODELAY`, as Kestrel does.
  - Security headers use static values.

## Where 5× is not reached, and why

- **Uncached searches by a single client:** 2.4× (1 row) to 4.9× (500 rows) on paired
  latency, and 3.8–5.6× on CPU.
  - Each such request includes the upstream round trip and four idle-process wake-ups
    (client, engine, upstream, engine). On this host those alone cost about 0.2 ms,
    which caps the ratio for tiny pages.
  - Above that floor, the cost is per-row HTML work. Rackett spends about 14 µs per
    ACG.RIP row; html5ever tokenizing and tree building is about a third of that.
    Jackett spends 80–95 µs per row.
  - Closing the rest needs a cheaper parse than a full html5ever DOM. That is a
    fidelity question, because AngleSharp's tree shape is what definitions are written
    against. It was left for a separate decision.
- **caps, paired:** 4.7× at the median (5.0× throughput, 22.6× CPU). An isolated caps
  request is mostly the idle wake-up; back to back it is 12.3×.
- **Memory after load:** 4.7–5.3× depending on the run. Most of it is allocator and
  per-connection buffer retention from 64 concurrent 135 KB responses. Setting
  mimalloc's purge delay to zero cut it to about 33 MiB, but cost 8–10% on uncached
  searches, so the default stays.
- **Rate-limited sources:** a cold 1337x search waits on its definition's
  `requestDelay: 3` for four pages in both engines (about 9 s). That is the site's
  limit, not data handling, and Rackett honours it exactly as Jackett does.

## Next steps

- Split the cold per-row cost into parse, select and render in the benchmark output.
  Try a reusable html5ever tree sink, and counting row descendants once per row
  instead of once per field.
- Add other source shapes to the benchmark, such as JSON APIs, login sessions and
  multi-page definitions with no delay. All of them share the paths optimized here,
  but none has been measured against Jackett yet.
- Re-run on a host with the `performance` governor to remove clock-scaling noise.
