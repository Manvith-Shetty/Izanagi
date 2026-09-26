//! Tab's HTTP surface: the website's API, the MCP server, and the website itself.
//!
//!   public       GET  /api/health, /api/network, /api/catalog, /api/approvals/{id}
//!                POST /api/signup, GET /api/signup/{id}      (World ID → your own wallet)
//!   signed in    GET  /api/me, /api/stream                   (your Tab, live)
//!   (cookie)     POST /api/buy, /api/approvals/{id}/wait, /api/tabs/close, /api/tabs/reopen,
//!                     /api/logout
//!   your agent   /mcp/{token}                                (Streamable HTTP MCP)
//!   everything else is the website (a single-page app).

use crate::app::Shared;
use crate::mcp::TabMcp;
use crate::store::Account;
use alloy::primitives::Address;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, Stream, StreamExt};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::services::{ServeDir, ServeFile};

const COOKIE: &str = "tab_session";

pub fn router(app: Shared) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/network", get(network))
        .route("/api/catalog", get(catalog))
        .route("/api/signup", post(signup))
        .route("/api/signup/{id}", get(signup_poll))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/stream", get(stream))
        .route("/api/buy", post(buy))
        .route("/api/approvals/{id}", get(approval))
        .route("/api/approvals/{id}/wait", post(approval_wait))
        .route("/api/tabs/close", post(close))
        .route("/api/tabs/reopen", post(reopen))
        .with_state(app.clone());

    let web = ServeDir::new(&app.env.web_dir).fallback(ServeFile::new(app.env.web_dir.join("index.html")));
    api.route_service("/mcp/{token}", mcp_service(app.clone()))
        .route_service("/mcp", mcp_service(app))
        .fallback_service(web)
}

fn mcp_service(app: Shared) -> StreamableHttpService<TabMcp, LocalSessionManager> {
    // DNS-rebinding protection: only answer for the host Tab is published at (and loopback)
    let mut hosts = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Ok(u) = url::Url::parse(&app.env.public_url) {
        if let Some(h) = u.host_str() {
            hosts.push(h.to_string());
            if let Some(p) = u.port() {
                hosts.push(format!("{h}:{p}"));
            }
        }
    }
    StreamableHttpService::new(
        move || Ok(TabMcp::new(app.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_allowed_hosts(hosts),
    )
}

/* -------------------------------- sessions -------------------------------- */

fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

async fn signed_in(app: &Shared, headers: &HeaderMap) -> Result<Account, Response> {
    match session_token(headers) {
        Some(t) => app.store.session_account(&t).await.ok_or_else(unauthorized),
        None => Err(unauthorized()),
    }
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "sign in with World ID first"}))).into_response()
}

fn cookie(app: &Shared, value: &str, max_age: u64) -> HeaderValue {
    let secure = if app.env.public_url.starts_with("https://") { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}"))
        .expect("a valid cookie")
}

fn bad(status: StatusCode, e: impl std::fmt::Display) -> Response {
    (status, Json(json!({"error": format!("{e:#}")}))).into_response()
}

/* --------------------------------- public --------------------------------- */

async fn health(State(app): State<Shared>) -> Response {
    let brain = app.brain.health().await.ok();
    let agent_gas = app.chain.gas_balance(app.chain.agent_address).await.ok();
    let treasury_gas = app.chain.gas_balance(app.chain.treasury_address).await.ok();
    Json(json!({
        "status": "ok",
        "countersigner": brain,
        "chainId": app.env.chain_id,
        "fork": app.env.is_fork(),
        "agent": app.chain.agent_address,
        "agentGasWei": agent_gas.map(|g| g.to_string()),
        "treasury": app.chain.treasury_address,
        "treasuryGasWei": treasury_gas.map(|g| g.to_string()),
        "accounts": app.store.account_count().await,
        "trialsLeft": app.env.trial_max_accounts.saturating_sub(app.store.account_count().await),
    }))
    .into_response()
}

async fn network(State(app): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({ "census": app.census.get().await, "accounts": app.store.account_count().await }))
}

async fn catalog(State(app): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({ "listings": app.catalog.list(None).await }))
}

/// What a person is being asked to approve. Anyone holding the link may look; only the
/// wallet's own human can approve it.
async fn approval(State(app): State<Shared>, Path(id): Path<String>) -> Response {
    match app.brain.approval(&id).await {
        Ok(Some(v)) => {
            let seller = v.seller.to_lowercase();
            let service = app
                .catalog
                .list(None)
                .await
                .into_iter()
                .find(|l| l.seller.to_lowercase() == seller)
                .map(|l| l.name);
            let world = v.world_link().to_string();
            Json(json!({ "approval": v, "service": service, "worldUrl": world })).into_response()
        }
        Ok(None) => bad(StatusCode::NOT_FOUND, "no such approval"),
        Err(e) => bad(StatusCode::BAD_GATEWAY, e),
    }
}

/* --------------------------------- signup --------------------------------- */

async fn signup(State(app): State<Shared>) -> Response {
    match app.onboarding.start().await {
        Ok((id, stage)) => Json(json!({ "id": id, "stage": stage })).into_response(),
        Err(e) => bad(StatusCode::BAD_GATEWAY, e),
    }
}

async fn signup_poll(State(app): State<Shared>, Path(id): Path<String>) -> Response {
    let stage = match app.onboarding.poll(&id).await {
        Ok(s) => s,
        Err(e) => return bad(StatusCode::NOT_FOUND, e),
    };
    let mut res = Json(json!({ "id": id, "stage": stage })).into_response();
    if let crate::onboard::Stage::Ready { account, .. } = &stage {
        match app.store.new_session(account).await {
            Ok(t) => {
                res.headers_mut().insert(header::SET_COOKIE, cookie(&app, &t, 7 * 24 * 3600));
            }
            Err(e) => return bad(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    }
    res
}

async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(t) = session_token(&headers) {
        let _ = app.store.end_session(&t).await;
    }
    let mut res = Json(json!({"ok": true})).into_response();
    res.headers_mut().insert(header::SET_COOKIE, cookie(&app, "", 0));
    res
}

/* -------------------------------- signed in -------------------------------- */

async fn me(State(app): State<Shared>, headers: HeaderMap) -> Response {
    match signed_in(&app, &headers).await {
        Ok(a) => Json(app.overview(&a).await).into_response(),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct BuyBody {
    url: String,
}

/// Try a purchase from the dashboard, exactly as an agent would through MCP.
async fn buy(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<BuyBody>) -> Response {
    let a = match signed_in(&app, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    Json(app.buyer.buy(&a, &b.url, None).await).into_response()
}

async fn approval_wait(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let a = match signed_in(&app, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    Json(app.buyer.wait(&a, &id, Duration::from_secs(25), |_| {}).await).into_response()
}

#[derive(Deserialize)]
struct TabBody {
    seller: Address,
    #[serde(default)]
    reason: Option<String>,
}

async fn close(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<TabBody>) -> Response {
    let a = match signed_in(&app, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    match app.close_tab(&a, b.seller, b.reason.as_deref()).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(StatusCode::BAD_REQUEST, e),
    }
}

async fn reopen(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<TabBody>) -> Response {
    let a = match signed_in(&app, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    match app.reopen_tab(&a, b.seller).await {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => bad(StatusCode::BAD_REQUEST, e),
    }
}

/// This person's slice of the live feed: only events about their own wallet.
async fn stream(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let a = match signed_in(&app, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let mine = format!("{:#x}", a.wallet);
    let is_mine = {
        let mine = mine.clone();
        move |i: &crate::feed::Item| i.data.get("wallet").and_then(|w| w.as_str()).map(|w| w.to_lowercase() == mine).unwrap_or(false)
    };
    let backlog: Vec<_> = app.feed.since(0).await.into_iter().filter(|i| is_mine(i)).collect();
    let past = stream::iter(backlog).map(|i| Ok::<_, Infallible>(sse(&i)));
    let live = BroadcastStream::new(app.feed.subscribe()).filter_map(move |m| {
        let keep = m.ok().filter(|i| is_mine(i));
        async move { keep.map(|i| Ok::<_, Infallible>(sse(&i))) }
    });
    let s: std::pin::Pin<Box<dyn Stream<Item = Result<SseEvent, Infallible>> + Send>> = Box::pin(past.chain(live));
    Sse::new(s).keep_alive(KeepAlive::default()).into_response()
}

fn sse(i: &crate::feed::Item) -> SseEvent {
    SseEvent::default()
        .id(i.id.to_string())
        .event("item")
        .json_data(i)
        .unwrap_or_else(|_| SseEvent::default().comment("unserialisable"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_cookie_is_found_among_others() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; tab_session=ses_abc; b=2"));
        assert_eq!(session_token(&h).as_deref(), Some("ses_abc"));
        h.insert(header::COOKIE, HeaderValue::from_static("a=1"));
        assert_eq!(session_token(&h), None);
    }
}
