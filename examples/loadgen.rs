//! Closed-loop HTTP load generator for `tools/benchmark_speed.py`.
//!
//! `C` workers each keep one keep-alive connection busy, one request at a time, until
//! `N` timed requests have completed. Every response must be 200 and, if `--expect`
//! is given, contain exactly that many `<item>` elements; anything else counts as an
//! error and fails the run. `{n}` in the URL becomes a process-wide counter, so a
//! template such as `q=probe{n}` makes every request a distinct (uncached) query.
//!
//! Prints one JSON object: latency percentiles in milliseconds, throughput and bytes.
//!
//! Given several `--url`s (concurrency 1 only), requests alternate between them, one
//! each per round with the starting URL rotating, and `{"paired": [...]}` holds one
//! report per URL. Engines compared this way see the same host state request by
//! request; a CPU clocked down between runs cannot favour either.
//!
//! `loadgen serve PORT` instead serves the synthetic upstream site: 1337x and ACG.RIP
//! listing pages (`/x/…` and `/?term=`) whose titles echo the search
//! term (a term starting `rN` returns N rows, otherwise 20), and `/__hits` reports how many listing pages were served.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Args {
    urls: Vec<String>,
    concurrency: usize,
    requests: usize,
    warmup: usize,
    expect: Option<usize>,
}

fn args() -> Args {
    let mut a = Args {
        urls: Vec::new(),
        concurrency: 1,
        requests: 1000,
        warmup: 100,
        expect: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--url" => a.urls.push(value),
            "--concurrency" => a.concurrency = value.parse().expect("concurrency"),
            "--requests" => a.requests = value.parse().expect("requests"),
            "--warmup" => a.warmup = value.parse().expect("warmup"),
            "--expect" => a.expect = Some(value.parse().expect("expect")),
            _ => panic!("unknown flag {flag}"),
        }
    }
    assert!(!a.urls.is_empty(), "--url is required");
    assert!(a.concurrency >= 1 && a.requests >= a.concurrency);
    assert!(
        a.urls.len() == 1 || a.concurrency == 1,
        "paired URLs need concurrency 1"
    );
    a
}

/// One request: the body length and the time until the whole body had arrived. The
/// response check that follows is not timed.
async fn one(
    client: &reqwest::Client,
    url: &str,
    expect: Option<usize>,
) -> Result<(usize, f64), String> {
    let started = Instant::now();
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    let elapsed = started.elapsed().as_secs_f64() * 1e3;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    if let Some(n) = expect {
        let items = body.windows(6).filter(|w| w == b"<item>").count();
        if items != n {
            return Err(format!("{items} items, expected {n}"));
        }
    }
    Ok((body.len(), elapsed))
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    let rank = ((sorted.len() as f64) * p / 100.0).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(1)
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap()
}
fn report(
    concurrency: usize,
    mut samples: Vec<f64>,
    errors: &[String],
    seconds: f64,
    bytes: usize,
) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    serde_json::json!({
        "concurrency": concurrency,
        "requests": samples.len(),
        "errors": errors.len(),
        "first_error": errors.first(),
        "seconds": seconds,
        "rps": samples.len() as f64 / seconds,
        "mean_ms": mean,
        "p50_ms": samples.first().map(|_| pct(&samples, 50.0)),
        "p90_ms": samples.first().map(|_| pct(&samples, 90.0)),
        "p95_ms": samples.first().map(|_| pct(&samples, 95.0)),
        "p99_ms": samples.first().map(|_| pct(&samples, 99.0)),
        "max_ms": samples.last(),
        "response_bytes": bytes,
    })
}
/// Claim one of the remaining requests, if any are left.
fn take(remaining: &AtomicUsize) -> bool {
    let mut n = remaining.load(Ordering::Relaxed);
    while n > 0 {
        match remaining.compare_exchange_weak(n, n - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(current) => n = current,
        }
    }
    false
}
fn fill(counter: &AtomicUsize, template: &str) -> String {
    if template.contains("{n}") {
        template.replace("{n}", &counter.fetch_add(1, Ordering::Relaxed).to_string())
    } else {
        template.to_owned()
    }
}
/// Alternating requests across `a.urls`; each URL's throughput is its requests over
/// the time spent on its own requests.
async fn paired(a: &Args) {
    let counter = AtomicUsize::new(0);
    let clients: Vec<_> = a.urls.iter().map(|_| client()).collect();
    for (client, template) in clients.iter().zip(&a.urls) {
        for _ in 0..a.warmup.max(1) {
            if let Err(e) = one(client, &fill(&counter, template), a.expect).await {
                eprintln!("warm-up failed: {e}");
                std::process::exit(2);
            }
        }
    }
    let n = a.urls.len();
    let mut samples = vec![Vec::new(); n];
    let mut errors = vec![Vec::new(); n];
    let mut bytes = vec![0; n];
    for round in 0..a.requests {
        for k in 0..n {
            let i = (round + k) % n;
            match one(&clients[i], &fill(&counter, &a.urls[i]), a.expect).await {
                Ok((len, elapsed)) => {
                    samples[i].push(elapsed);
                    bytes[i] = len;
                }
                Err(e) => errors[i].push(e),
            }
        }
    }
    let reports: Vec<_> = (0..n)
        .map(|i| {
            let seconds = samples[i].iter().sum::<f64>() / 1e3;
            report(
                1,
                std::mem::take(&mut samples[i]),
                &errors[i],
                seconds,
                bytes[i],
            )
        })
        .collect();
    println!("{}", serde_json::json!({ "paired": reports }));
    if errors.iter().any(|e| !e.is_empty()) {
        std::process::exit(1);
    }
}

fn rows(source: &str, term: &str, count: usize) -> String {
    let term = html_escape::encode_text(term);
    let mut html = String::from("<html><body><table><tbody>");
    for n in 0..count {
        let title = format!("Linux Fixture {term} {n:03}");
        html.push_str(&if source == "1337x" {
            format!(r#"<tr><td class="coll-1"><a href="/sub/42/0/">category</a><a href="/torrent/{n}/{title}/">{title}</a></td><td class="coll-2">5</td><td class="coll-3">1</td><td class="coll-date">Apr. 18th '11</td><td class="coll-4">1 MB</td><td class="coll-5">Fixture</td></tr>"#)
        } else {
            format!(r#"<tr><td class="title"><span class="title"><a href="/details/{n}">{title}</a></span></td><td class="action"><a href="/torrent/{n}">Download</a></td><td class="size">1 MB</td><td class="date"><time datetime="2026-10-01T00:00:00Z"></time></td></tr>"#)
        });
    }
    html + "</tbody></table></body></html>"
}

async fn serve(port: u16) {
    use axum::{
        extract::{Path, Query},
        routing::get,
        Router,
    };
    use std::collections::HashMap;
    let hits = Arc::new(AtomicUsize::new(0));
    // A term starting `rN` (r100x7) asks for N rows; anything else gets 20.
    let page = |source: &'static str, term: String, hits: &AtomicUsize| {
        hits.fetch_add(1, Ordering::Relaxed);
        let digits: String = term
            .strip_prefix('r')
            .unwrap_or("")
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let count = digits.parse().unwrap_or(20usize).min(2000);
        axum::response::Html(rows(source, &term, count))
    };
    let (x, a, counter) = (hits.clone(), hits.clone(), hits.clone());
    let app = Router::new()
        .route(
            "/__hits",
            get(move || async move { counter.load(Ordering::Relaxed).to_string() }),
        )
        // 1337x carries the term in the path: /x/sort-search/TERM/time/desc/1/.
        .route(
            "/x/{*path}",
            get(move |Path(path): Path<String>| async move {
                let term = path.split('/').nth(1).unwrap_or("").replace('+', " ");
                page("1337x", term, &x)
            }),
        )
        .route(
            "/",
            get(move |Query(q): Query<HashMap<String, String>>| async move {
                page("acgrip", q.get("term").cloned().unwrap_or_default(), &a)
            }),
        );
    let listener = axum::serve::ListenerExt::tap_io(
        tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap(),
        |tcp| {
            let _ = tcp.set_nodelay(true);
        },
    );
    axum::serve(listener, app).await.unwrap();
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("serve") {
        let port = std::env::args()
            .nth(2)
            .and_then(|p| p.parse().ok())
            .expect("serve PORT");
        return serve(port).await;
    }
    let a = args();
    if a.urls.len() > 1 {
        return paired(&a).await;
    }
    let counter = Arc::new(AtomicUsize::new(0));
    let clients: Vec<_> = (0..a.concurrency).map(|_| client()).collect();
    // Warm-up: opens every connection and fills caches; never timed.
    for (i, client) in clients.iter().enumerate() {
        for _ in 0..(a.warmup / a.concurrency).max(usize::from(i == 0)) {
            if let Err(e) = one(client, &fill(&counter, &a.urls[0]), a.expect).await {
                eprintln!("warm-up failed: {e}");
                std::process::exit(2);
            }
        }
    }
    let remaining = Arc::new(AtomicUsize::new(a.requests));
    let started = Instant::now();
    let workers: Vec<_> = clients
        .into_iter()
        .map(|client| {
            let (remaining, counter, template) =
                (remaining.clone(), counter.clone(), a.urls[0].clone());
            let expect = a.expect;
            tokio::spawn(async move {
                let (mut samples, mut errors, mut bytes) = (Vec::new(), Vec::new(), 0usize);
                while take(&remaining) {
                    match one(&client, &fill(&counter, &template), expect).await {
                        Ok((n, elapsed)) => {
                            samples.push(elapsed);
                            bytes = n;
                        }
                        Err(e) => errors.push(e),
                    }
                }
                (samples, errors, bytes)
            })
        })
        .collect();
    let (mut samples, mut errors, mut bytes) = (Vec::new(), Vec::new(), 0);
    for w in workers {
        let (s, e, b) = w.await.unwrap();
        samples.extend(s);
        errors.extend(e);
        bytes = bytes.max(b);
    }
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "{}",
        report(a.concurrency, samples, &errors, seconds, bytes)
    );
    if !errors.is_empty() {
        std::process::exit(1);
    }
}
