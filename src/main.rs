use axum::{http::header, response::Html, routing::get, Router};
use rackett::{engine::Engine, AppState};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli_args: Vec<_> = std::env::args().skip(1).collect();
    if cli_args.first().is_some_and(|s| s == "definitions") {
        return definitions_cli(&cli_args[1..]);
    }
    let mut data = std::env::var_os("RACKETT_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "data".into());
    let mut port = None;
    let mut public_url = std::env::var("RACKETT_PUBLIC_URL").ok();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("Rackett {}\n\nUsage: rackett [--port PORT] [--data-dir PATH] [--public-url HTTPS_ORIGIN]\n       rackett definitions check PATH [--json]\n\n  --port PORT       Listen on 127.0.0.1:PORT (default 9117)\n  --data-dir PATH   Private persistent configuration directory\n                    (default RACKETT_DATA_DIR or ./data)\n  --public-url URL  Trusted HTTPS reverse-proxy origin (or RACKETT_PUBLIC_URL)\n  --version        Print version\n\nManagement writes require a same-origin Origin header.\nThe listener stays on loopback; a configured HTTPS proxy may serve the tailnet.\nFull legacy tracker parity is not supported.",env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--version" | "-V" => {
                println!("rackett {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--port" => {
                let p = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--port requires a value"))?
                    .parse::<u16>()?;
                if p == 0 {
                    anyhow::bail!("Port must be greater than zero")
                }
                port = Some(p)
            }
            "--public-url" => {
                public_url = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--public-url requires a value"))?,
                );
            }
            "--data-dir" => {
                data = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--data-dir requires a value"))?
                    .into()
            }
            _ => anyhow::bail!("Unknown option {arg}; use --help"),
        }
    }
    let engine = std::sync::Arc::new(Engine::new(data.join("config.json")).await?);
    if let Some(port) = port {
        engine.set_port(port).await?;
    }
    if let Some(url) = public_url {
        engine.set_public_url(&url).await?;
    }
    let settings = engine.settings().await;
    let app = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/app.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/icon.svg",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/svg+xml")],
                    include_str!("../assets/icon.svg"),
                )
            }),
        )
        .route("/health", get(|| async { "Rackett ready" }))
        .merge(rackett::api::router_with_public_url(
            settings.public_url.clone(),
        ))
        .merge(rackett::protocol::router())
        .with_state(AppState { engine })
        .layer(axum::middleware::from_fn(security_headers));
    let address = format!("127.0.0.1:{}", settings.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Rackett listening at http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}
async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    for (name,value) in [
        ("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"),
        ("x-content-type-options","nosniff"),
        ("referrer-policy","no-referrer"),
        ("cache-control","no-store"),
        ("x-frame-options","DENY"),
    ] { response.headers_mut().insert(name,value.parse().expect("static header")); }
    response
}
async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        let terminate = async {
            if let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                signal.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {_=interrupt=>{},_=terminate=>{}}
    }
    #[cfg(not(unix))]
    interrupt.await;
}

fn definitions_cli(args: &[String]) -> anyhow::Result<()> {
    if args.len() < 2
        || args[0] != "check"
        || args.len() > 3
        || (args.len() == 3 && args[2] != "--json")
    {
        anyhow::bail!("Usage: rackett definitions check PATH [--json]");
    }
    let report =
        rackett::cardigann::check_path(std::path::Path::new(&args[1]), Default::default())?;
    if args.len() == 3 {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for definition in &report.definitions {
            println!(
                "{}: {} (execution unsupported)",
                definition.file,
                if definition.model_valid {
                    "model valid"
                } else {
                    "INVALID"
                }
            );
            for d in &definition.diagnostics {
                if d.severity != rackett::cardigann::Severity::Info {
                    println!("  {:?} {} {}: {}", d.severity, d.code, d.path, d.message);
                }
            }
        }
        println!("{} definitions: {} valid, {} invalid; 0 executable (Cardigann interpreter unimplemented)", report.summary.total,report.summary.valid,report.summary.invalid);
    }
    if report.summary.invalid != 0 || report.summary.total == 0 {
        std::process::exit(1);
    }
    Ok(())
}
