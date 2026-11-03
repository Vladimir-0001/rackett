//! In-process timing of Cardigann row extraction for `docs/speed.md`: the pinned
//! ACG.RIP and 1337x plans over the same synthetic listing pages that `loadgen serve`
//! returns, without HTTP, caching or presentation. Prints microseconds per page and row.
//!
//! `cargo run --release --example extract_bench [ROWS] [ITERATIONS] [SOURCE]`
use rackett::{builtin::Runtime, models::Indexer, models::SearchQuery};
use std::time::Instant;

fn page(source: &str, count: usize) -> String {
    let mut html = String::from("<html><body><table><tbody>");
    for n in 0..count {
        let title = format!("Linux Fixture bench {n:03}");
        html.push_str(&if source == "1337x" {
            format!(r#"<tr><td class="coll-1"><a href="/sub/42/0/">category</a><a href="/torrent/{n}/{title}/">{title}</a></td><td class="coll-2">5</td><td class="coll-3">1</td><td class="coll-date">Apr. 18th '11</td><td class="coll-4">1 MB</td><td class="coll-5">Fixture</td></tr>"#)
        } else {
            format!(r#"<tr><td class="title"><span class="title"><a href="/details/{n}">{title}</a></span></td><td class="action"><a href="/torrent/{n}">Download</a></td><td class="size">1 MB</td><td class="date"><time datetime="2026-10-01T00:00:00Z"></time></td></tr>"#)
        });
    }
    html + "</tbody></table></body></html>"
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rows: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(100);
    let iterations: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(200);
    let only = args.next();
    for source in ["acgrip", "1337x"] {
        if only.as_deref().is_some_and(|o| o != source) {
            continue;
        }
        let indexer = Indexer {
            id: "bench".into(),
            name: source.into(),
            kind: "cardigann".into(),
            url: "http://127.0.0.1:9/".into(),
            definition: Some(serde_json::json!({"builtin_id": source, "settings": {}})),
            ..Default::default()
        };
        let runtime = Runtime::compile(&indexer).expect("ready definition");
        let query = SearchQuery {
            params: [("q", "bench"), ("t", "search")]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        };
        let ctx = runtime
            .plan
            .context(&runtime.settings, &query, chrono::Utc::now());
        let request = runtime.plan.build_requests(&ctx).unwrap().remove(0);
        let body = page(source, rows);
        let found = runtime.plan.extract(&request, &body, &ctx).unwrap().len();
        assert_eq!(found, rows, "{source}");
        let started = Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(runtime.plan.extract(&request, &body, &ctx).unwrap());
        }
        let per_page = started.elapsed().as_secs_f64() * 1e6 / iterations as f64;
        println!(
            "{source:7} {rows} rows: {per_page:9.1} us/page {:6.2} us/row",
            per_page / rows as f64
        );
    }
}
