# Date header rows (`rows.dateheaders`)

Some trackers list releases under day headings ("Torrents added Monday 3 March") rather
than giving every row its own date. Cardigann's `rows.dateheaders` block reads that
heading. Rackett runs it as pinned Jackett does (`CardigannIndexer.cs:1910-1946`, commit
`fe8876d2`).

## Behaviour

- It applies only to HTML and XML rows that end up with no publish date after their
  fields, for example a definition without a `date` field. Rows with their own date
  keep it. JSON responses ignore the block, as upstream's JSON branch does.
- Starting at the row, Rackett walks to the previous element sibling. When a level has
  none, it moves to the previous element sibling of that element's parent. It stops at
  the first element where the block selects a value. The walk only moves backwards or
  upwards (`selector::previous_header_slot`).
- The block is a normal selector block: `selector`, `attribute`, `filters` and
  `optional`. Like `handleSelector`, it tests the element itself before its
  descendants, so a header row that matches the selector (cyclingarchiveclub's
  `:has(...)`) is found.
- A selector miss or a filter error moves on to the next earlier element. A selected
  value is final: it is parsed like a `date` field (`FromUnknown`). If it doesn't parse,
  the row fails. Note that `dateparse` returns unparsable input unchanged, as upstream
  does.
- With no header found, a required block fails the row with
  `$.search.rows.dateheaders: No date header row found`. An `optional: true` block
  leaves the row undated.
- Row failures follow Rackett's existing field policy. Upstream logs and skips a failed
  row, while Rackett fails the response unless the caller extracts with
  invalid-row skipping.
- Other block keys (`remove`, `case`, `text`, `default`) are rejected at compile time.
  No pinned definition uses them here.

## Effect on the bundled definitions

boxingtorrents, cyclingarchiveclub and hdgalaktik now compile their search plan. Each
still needs its login family and fixtures before it is ready:

| ID | Login | Next step |
| --- | --- | --- |
| boxingtorrents | cookie (`CookiePlan` compiles) | cookie fixture case |
| cyclingarchiveclub | form (`FormPlan` compiles) | form fixture case |
| hdgalaktik | form | form sessions with its non-GET search shape |

## Tests

`tests/cardigann_dateheaders.rs` covers nearest-header selection across several rows,
the parent climb, self-matching header rows, precedence of a row's own date, required
and optional misses, the "nearest match wins" rule and rejected keys.
