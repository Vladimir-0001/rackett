use crate::{
    models::{Indexer, SearchQuery},
    AppState,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn err(e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":e.to_string()})),
    )
}
pub fn router() -> Router<AppState> {
    router_with_public_url(None)
}
pub fn router_with_public_url(public_url: Option<String>) -> Router<AppState> {
    Router::new()
        .route("/api/dashboard", get(dashboard))
        .route("/api/indexers", get(indexers).post(create))
        .route(
            "/api/indexers/{id}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/api/indexers/{id}/test", post(test))
        .route("/api/catalog", get(catalog))
        .route("/api/search", get(search))
        .route("/api/settings", get(settings).patch(patch_settings))
        .route("/api/settings/rotate-key", post(rotate))
        .route("/api/activity", get(activity))
        .route("/api/cache", delete(clear))
        .layer(middleware::from_fn(move |req, next| {
            let public_url = public_url.clone();
            async move { guard_origin(req, next, public_url.as_deref()).await }
        }))
}
pub async fn guard(req: Request<Body>, next: Next) -> Response {
    guard_origin(req, next, None).await
}
async fn guard_origin(req: Request<Body>, next: Next, public_url: Option<&str>) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let parsed = url::Url::parse(&format!("http://{host}"));
    let local = parsed
        .as_ref()
        .ok()
        .and_then(|u| u.host_str())
        .is_some_and(|s| s == "localhost" || s == "127.0.0.1" || s == "[::1]" || s == "::1");
    let public_authority = public_url
        .and_then(|v| url::Url::parse(v).ok())
        .map(|u| u[url::Position::BeforeHost..url::Position::AfterPort].to_string());
    let trusted = public_authority.as_deref() == Some(host);
    if !local && !trusted {
        return (StatusCode::FORBIDDEN, "Trusted Host header required").into_response();
    }
    let local_origin = format!("http://{host}");
    let origin = req.headers().get("origin").and_then(|h| h.to_str().ok());
    if origin.is_some_and(|o| {
        o != local_origin && Some(o) != public_url || (trusted && Some(o) != public_url)
    }) || req
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return (StatusCode::FORBIDDEN, "Same-origin access required").into_response();
    }
    if req.method() != axum::http::Method::GET
        && req.method() != axum::http::Method::HEAD
        && origin.is_none()
    {
        return (
            StatusCode::FORBIDDEN,
            "Origin header required for management changes",
        )
            .into_response();
    }
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
}
async fn public_indexers(s: &AppState) -> Vec<Value> {
    let statuses = s.engine.statuses.read().await;
    s.engine
        .indexers()
        .await
        .into_iter()
        .map(|mut i| {
            let original_url = i.url.clone();
            if let Ok(mut url) = url::Url::parse(&i.url) {
                if url.query().is_some() {
                    let names: Vec<String> =
                        url.query_pairs().map(|(k, _)| k.into_owned()).collect();
                    url.query_pairs_mut()
                        .clear()
                        .extend_pairs(names.iter().map(|k| (k.as_str(), "REDACTED")));
                    i.url = url.to_string();
                }
            }
            let url_redacted = i.url != original_url;
            i.api_key.clear();
            i.cookie.clear();
            let mut v = serde_json::to_value(&i).unwrap();
            v.as_object_mut().unwrap().remove("api_key");
            v.as_object_mut().unwrap().remove("cookie");
            v.as_object_mut().unwrap().remove("definition");
            if let Some(d) = crate::builtin::public_definition(&i) {
                v["definition"] = d;
            }
            let status = statuses.get(&i.id);
            v["url_redacted"] = json!(url_redacted);
            v["status"] = json!(status.map(|s| s.status.as_str()).unwrap_or("untested"));
            v["latency_ms"] = json!(status.and_then(|s| s.latency_ms));
            v["last_error"] = json!(status.and_then(|s| s.last_error.clone()));
            v
        })
        .collect()
}
async fn dashboard(State(s): State<AppState>) -> Json<Value> {
    let mut stats = s.engine.stats();
    stats["cache_entries"] = json!(s.engine.cache_entries().await);
    Json(
        json!({"indexers":public_indexers(&s).await,"stats":stats,"activity":s.engine.activity().await}),
    )
}
async fn indexers(State(s): State<AppState>) -> Json<Value> {
    Json(json!(public_indexers(&s).await))
}
async fn catalog() -> Json<Value> {
    Json(crate::adapters::catalog())
}
async fn create(State(s): State<AppState>, Json(i): Json<Indexer>) -> ApiResult {
    let i = s.engine.put_indexer(i, false).await.map_err(err)?;
    Ok(Json(json!({"id":i.id,"name":i.name,"ok":true})))
}
async fn update(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> ApiResult {
    let i = s.engine.patch_indexer(&id, patch).await.map_err(err)?;
    Ok(Json(json!({"ok":true,"id":i.id})))
}
async fn remove(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult {
    s.engine.delete_indexer(&id).await.map_err(err)?;
    Ok(Json(json!({"ok":true})))
}
async fn test(State(s): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    match s.engine.test(&id).await {
        Ok(ms) => {
            Json(json!({"ok":true,"latency_ms":ms,"message":"Source responded successfully"}))
        }
        Err(e) => Json(json!({"ok":false,"message":e.to_string()})),
    }
}
async fn search(
    State(s): State<AppState>,
    Query(params): Query<std::collections::BTreeMap<String, String>>,
) -> ApiResult {
    crate::protocol::validate_query(&params).map_err(err)?;
    let ids: Vec<String> = params
        .get("indexers")
        .map(|s| {
            s.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let selected: Vec<_> = s
        .engine
        .indexers()
        .await
        .into_iter()
        .filter(|i| {
            i.enabled && (ids.is_empty() || ids.iter().any(|id| id == "all" || id == &i.id))
        })
        .collect();
    crate::protocol::validate_source_ids(&params, &selected).map_err(err)?;
    let mut response = s.engine.search(ids, SearchQuery { params }).await;
    let settings = s.engine.settings().await;
    let links = s
        .engine
        .links(&response.results, &settings.api_key, &settings.base_url())
        .await;
    crate::protocol::present_results(&mut response, &links);
    Ok(Json(json!(response)))
}
async fn settings(State(s): State<AppState>) -> Json<Value> {
    Json(json!(s.engine.settings().await))
}
async fn patch_settings(State(s): State<AppState>, Json(v): Json<Value>) -> ApiResult {
    Ok(Json(json!(s
        .engine
        .update_settings(v)
        .await
        .map_err(err)?)))
}
async fn rotate(State(s): State<AppState>) -> ApiResult {
    Ok(Json(json!(s.engine.rotate_key().await.map_err(err)?)))
}
async fn activity(State(s): State<AppState>) -> Json<Value> {
    Json(json!(s.engine.activity().await))
}
async fn clear(State(s): State<AppState>) -> Json<Value> {
    s.engine.clear_cache().await;
    Json(json!({"ok":true}))
}
