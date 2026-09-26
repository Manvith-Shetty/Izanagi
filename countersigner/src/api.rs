//! The countersigner's HTTP API.
//!
//! Two audiences, two levels of trust:
//!
//!   agent-facing, open        POST /v1/countersign, POST /v1/approve/poll,
//!                             GET /v1/approvals/{id}, GET /health
//!   operator-facing, bearer   GET /v1/wallets/{wallet}, GET /v1/activity, GET /v1/stream,
//!   (`CONTROL_TOKEN`)         GET /v1/screen/{address}, POST /v1/revoke, POST /v1/restore,
//!                             POST /v1/enroll, POST /v1/enroll/{id}/claim, POST /v1/bind
//!
//! The agent can ask for anything and decide nothing. The operator API can stop payments
//! and ask a human to restart them; it cannot restart them itself.

use crate::app::Shared;
use crate::approvals::{restore_digest, NewApproval, Purpose, Status};
use crate::journal::{Actor, Event};
use crate::policy::{fmt_usdc, reason_code, Verdict};
use crate::signer;
use crate::state;
use alloy::primitives::{Address, B256};
use axum::{
    extract::{Path, Query, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use common::ChannelConfig;
use futures_util::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/countersign", post(countersign))
        .route("/v1/approve/poll", post(approve_poll))
        .route("/v1/approvals/{id}", get(approval))
        .route("/v1/sessions", get(sessions))
        .route("/v1/wallets/{wallet}", get(wallet))
        .route("/v1/activity", get(activity))
        .route("/v1/stream", get(stream))
        .route("/v1/screen/{address}", get(screen))
        .route("/v1/revoke", post(revoke))
        .route("/v1/restore", post(restore))
        .route("/v1/enroll", post(enroll))
        .route("/v1/enroll/{id}/claim", post(enroll_claim))
        .route("/v1/bind", post(bind))
        .with_state(app)
}

/* ------------------------------- operator auth ------------------------------- */

/// `Err` is the response to send: 404 when the operator API is off (it does not exist),
/// 401 when the token is wrong.
fn operator(app: &Shared, headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = &app.settings.control_token else {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "operator API disabled: set CONTROL_TOKEN"}))).into_response());
    };
    let given = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !constant_time_eq(given.as_bytes(), expected.as_bytes()) {
        return Err((StatusCode::UNAUTHORIZED, Json(json!({"error": "bad control token"}))).into_response());
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg.into()}))).into_response()
}

/* --------------------------------- agent-facing --------------------------------- */

async fn health(State(app): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "oracle": format!("{:#x}", app.oracle.address()),
        "humanLimit": fmt_usdc(app.settings.human_limit),
        "worldIdConfigured": app.world.is_some(),
        "phonePushConfigured": app.notify.is_some(),
        "requireOrb": app.world.as_ref().map(|w| w.require_orb),
        "attestationTtlSecs": app.settings.attestation_ttl,
        "onChainGuardian": app.guardian.is_some(),
        "operatorApi": app.settings.control_token.is_some(),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelBody {
    payer: Address,
    receiver: Address,
    receiver_authorizer: Address,
    token: Address,
    #[serde(default = "default_delay")]
    withdraw_delay: u64,
    #[serde(default)]
    salt: Option<String>,
}
fn default_delay() -> u64 {
    900
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountersignRequest {
    channel: ChannelBody,
    chain_id: u64,
    /// Cumulative ceiling the agent wants authorised.
    ceiling: u128,
    /// A human approval to redeem, from a previous `ask`. The agent cannot supply a subject
    /// directly: it could move one human's approval onto a different payment.
    #[serde(default)]
    approval_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingApproval {
    approval_id: String,
    user_code: String,
    verification_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_uri_complete: Option<String>,
    /// The page that shows a person exactly what they are approving.
    approval_url: String,
    expires_in: u64,
    interval: u64,
    seller: String,
    /// The limit the person is asked to approve for this tab.
    ceiling: u128,
    /// The cumulative amount the agent actually asked for now.
    requested: u128,
    /// The limit a person had already approved for this tab (zero if none).
    approved_so_far: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CountersignResponse {
    verdict: Verdict,
    reason: String,
    toxic_score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    countersignature: Option<signer::Countersignature>,
    /// Present when a human must approve first. Carries only what the agent needs to display
    /// and an opaque handle -- never the `device_code`, which is a credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    approval: Option<PendingApproval>,
    /// Everything the screening saw, so the flow can show the risk and not just the answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<serde_json::Value>,
}

fn refuse(code: StatusCode, reason: &str, score: f64) -> (StatusCode, Json<CountersignResponse>) {
    (
        code,
        Json(CountersignResponse {
            verdict: Verdict::Refuse,
            reason: reason.to_string(),
            toxic_score: score,
            countersignature: None,
            approval: None,
            evidence: None,
        }),
    )
}

fn verdict_name(v: &Verdict) -> &'static str {
    match v {
        Verdict::Pay { .. } => "pay",
        Verdict::Cap { .. } => "cap",
        Verdict::Ask { .. } => "ask",
        Verdict::Refuse => "refuse",
    }
}

/// The moment of decision.
async fn countersign(
    State(app): State<Shared>,
    Json(req): Json<CountersignRequest>,
) -> (StatusCode, Json<CountersignResponse>) {
    let salt = req
        .channel
        .salt
        .as_deref()
        .and_then(|s| s.trim_start_matches("0x").parse::<B256>().ok())
        .unwrap_or(B256::ZERO);
    let cfg = ChannelConfig::countersign(
        req.channel.payer,
        req.channel.receiver,
        req.channel.receiver_authorizer,
        req.channel.token,
        req.channel.withdraw_delay,
        salt,
    );
    let (wallet, seller_addr) = (req.channel.payer, req.channel.receiver);
    let seller = format!("{seller_addr:#x}");

    // a closed tab never gets another signature
    if app.sessions.is_revoked(wallet, seller_addr).await {
        return refuse(StatusCode::FORBIDDEN, "this tab is closed: the seller was revoked", -1.0);
    }
    // closed on chain by someone else (the owner, directly)? Then a voucher would be worthless.
    if let Some(g) = &app.guardian {
        if let Ok(s) = g.standing(wallet, seller_addr).await {
            if s.revoked || s.paused {
                let why = if s.paused { "the wallet is paused" } else { "the seller is revoked on chain" };
                return refuse(StatusCode::FORBIDDEN, why, -1.0);
            }
        }
    }

    // screen the voucher itself, built here rather than taken from the agent: it is exactly
    // what gets signed, since the countersignature commits to its digest
    let channel_id = common::channel_id(&cfg, req.chain_id);
    let voucher = common::voucher_typed_data(channel_id, req.ceiling, req.chain_id);
    let decision = app
        .policy
        .evaluate(
            &format!("{wallet:#x}"),
            &seller,
            &format!("{:#x}", req.channel.token),
            req.chain_id,
            req.ceiling,
            Some(&voucher),
        )
        .await;
    let cid = format!("{channel_id:#x}");
    let approved = app.sessions.approved_limit(&cid).await;
    let within_approved = matches!(decision.verdict, Verdict::Ask { ceiling, .. } if ceiling <= approved);

    app.journal
        .record(Event::Screened {
            wallet: format!("{wallet:#x}"),
            seller: seller.clone(),
            verdict: if within_approved { "pay" } else { verdict_name(&decision.verdict) }.into(),
            reason: if within_approved {
                format!("within the {} a person approved for this tab", fmt_usdc(approved))
            } else {
                decision.reason.clone()
            },
            toxic_score: decision.toxic_score,
            requested: req.ceiling,
        })
        .await;

    let evidence = Some(json!({
        "addressScan": decision.address_scan,
        "tokenScan": decision.token_scan,
        "messageScan": decision.message_scan,
    }));

    let allowed = match &decision.verdict {
        Verdict::Pay { ceiling } | Verdict::Cap { ceiling, .. } => *ceiling,
        Verdict::Refuse => {
            return (
                StatusCode::FORBIDDEN,
                Json(CountersignResponse {
                    verdict: Verdict::Refuse,
                    reason: decision.reason,
                    toxic_score: decision.toxic_score,
                    countersignature: None,
                    approval: None,
                    evidence,
                }),
            );
        }
        // A person already approved this tab up to a limit, and this call fits under it.
        // Screening still ran above: a score that crossed `refuse_at` was refused regardless.
        Verdict::Ask { ceiling, .. } if *ceiling <= approved => *ceiling,
        Verdict::Ask { ceiling, .. } => {
            // A person approves raising THIS tab's limit, not one voucher: the digest commits to
            // the channel (payer, seller, token) and the exact limit, so the approval cannot be
            // moved to another seller or amount, and it is single use.
            let limit = state::approval_limit(*ceiling, app.settings.approval_step);
            let digest = format!("{:#x}", common::voucher_digest(channel_id, limit, req.chain_id));
            match &req.approval_id {
                Some(id) => match app.approvals.consume(id, &digest).await {
                    Ok(sub) => {
                        let delta = limit.saturating_sub(approved);
                        if !app.within_budget(&sub, delta).await {
                            return refuse(
                                StatusCode::FORBIDDEN,
                                &format!(
                                    "this person's {} budget is already spent across their agents",
                                    fmt_usdc(app.settings.human_limit)
                                ),
                                decision.toxic_score,
                            );
                        }
                        app.charge(&sub, delta).await;
                        app.sessions.raise_limit(&cid, limit).await;
                        *ceiling
                    }
                    Err(e) => return refuse(StatusCode::FORBIDDEN, &e.to_string(), decision.toxic_score),
                },
                None => {
                    let n = NewApproval {
                        purpose: Purpose::Payment,
                        wallet,
                        seller: seller_addr,
                        amount: limit,
                        reason: decision.reason.clone(),
                        bound_digest: digest,
                    };
                    let a = match app.request_approval(n).await {
                        Ok(a) => a,
                        Err(e) => {
                            return refuse(
                                StatusCode::BAD_GATEWAY,
                                &format!("could not start human approval, refusing: {e:#}"),
                                decision.toxic_score,
                            )
                        }
                    };
                    let approval_url = app.approval_link(&a);
                    return (
                        StatusCode::ACCEPTED,
                        Json(CountersignResponse {
                            verdict: decision.verdict.clone(),
                            reason: decision.reason.clone(),
                            toxic_score: decision.toxic_score,
                            countersignature: None,
                            approval: Some(PendingApproval {
                                approval_id: a.id,
                                user_code: a.user_code,
                                verification_uri: a.verification_uri,
                                verification_uri_complete: a.verification_uri_complete,
                                approval_url,
                                expires_in: a.expires_at.saturating_sub(a.created_at),
                                interval: a.interval,
                                seller,
                                ceiling: limit,
                                requested: *ceiling,
                                approved_so_far: approved,
                            }),
                            evidence,
                        }),
                    );
                }
            }
        }
    };

    let expiry = state::now() + app.settings.attestation_ttl;
    match app.oracle.countersign(&cfg, req.chain_id, allowed, expiry) {
        Ok(sig) => {
            app.sessions
                .upsert(&sig.channel_id, wallet, seller_addr, req.channel.token, req.chain_id, allowed, expiry, decision.toxic_score)
                .await;
            app.journal
                .record(Event::Countersigned {
                    wallet: format!("{wallet:#x}"),
                    seller: seller.clone(),
                    channel_id: sig.channel_id.clone(),
                    ceiling: allowed,
                    expiry,
                })
                .await;
            (
                StatusCode::OK,
                Json(CountersignResponse {
                    verdict: decision.verdict,
                    reason: decision.reason,
                    toxic_score: decision.toxic_score,
                    countersignature: Some(sig),
                    approval: None,
                    evidence,
                }),
            )
        }
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, &format!("signing failed: {e}"), decision.toxic_score),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollBody {
    /// The opaque handle from the `ask` response. The `device_code` stays server-side.
    approval_id: String,
}

/// Where an approval has got to. World is polled in the background; this only reads.
/// 200 approved (or used) · 202 pending · 403 denied or expired · 404 unknown.
async fn approve_poll(State(app): State<Shared>, Json(body): Json<PollBody>) -> Response {
    let Some(a) = app.approvals.get(&body.approval_id).await else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "no such approval"}))).into_response();
    };
    let v = a.view();
    match v.status {
        Status::Pending => (StatusCode::ACCEPTED, Json(json!({"status": "pending"}))).into_response(),
        // deliberately NOT returning the subject: the agent has no use for it, and holding it
        // is what would let an approval be carried to another payment
        Status::Approved | Status::Used => (
            StatusCode::OK,
            Json(json!({"status": v.status, "orbVerified": v.orb_verified.unwrap_or(false)})),
        )
            .into_response(),
        Status::Denied => (
            StatusCode::FORBIDDEN,
            Json(json!({"status": "denied", "reason": v.denied_reason})),
        )
            .into_response(),
        Status::Expired => (
            StatusCode::FORBIDDEN,
            Json(json!({"status": "denied", "reason": "expired_token"})),
        )
            .into_response(),
    }
}

/// What a person is being asked to approve. Safe to show anyone holding the link: it carries
/// no credential, and approving it still takes the wallet's own human.
async fn approval(State(app): State<Shared>, Path(id): Path<String>) -> Response {
    match app.approvals.get(&id).await {
        Some(a) => Json(a.view()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "no such approval"}))).into_response(),
    }
}

/// Kept for the CLI agent: open sessions and the revocations so far.
async fn sessions(State(app): State<Shared>) -> Json<serde_json::Value> {
    let revocations: Vec<_> = app
        .journal
        .since(0)
        .await
        .into_iter()
        .filter(|e| matches!(e.event, Event::Revoked { .. }))
        .collect();
    Json(json!({ "sessions": app.sessions.list().await, "revocations": revocations }))
}

/* -------------------------------- operator-facing -------------------------------- */

/// One wallet, as the countersigner sees it.
async fn wallet(State(app): State<Shared>, headers: HeaderMap, Path(wallet): Path<Address>) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    let guardian = match &app.guardian {
        Some(g) => json!({
            "oracle": format!("{:#x}", g.oracle()),
            "guards": g.guards(wallet).await.ok(),
        }),
        None => serde_json::Value::Null,
    };
    Json(json!({
        "wallet": format!("{wallet:#x}"),
        "human": app.bindings.human_of(wallet).await,
        "guardian": guardian,
        "sessions": app.sessions.for_wallet(wallet).await,
        "closedSellers": app.sessions.blocked_for(wallet).await,
        "approvals": app.approvals.pending_for(wallet).await,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct After {
    #[serde(default)]
    after: u64,
}

async fn activity(State(app): State<Shared>, headers: HeaderMap, Query(q): Query<After>) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    Json(json!({ "entries": app.journal.since(q.after).await })).into_response()
}

/// The journal, live. A subscriber that falls behind is told how far, rather than silently
/// missing entries, and can catch up from `/v1/activity`.
async fn stream(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    let live = BroadcastStream::new(app.journal.subscribe()).map(|m| {
        Ok::<_, Infallible>(match m {
            Ok(entry) => SseEvent::default()
                .id(entry.id.to_string())
                .event("entry")
                .json_data(&entry)
                .unwrap_or_else(|_| SseEvent::default().comment("unserialisable entry")),
            Err(BroadcastStreamRecvError::Lagged(n)) => SseEvent::default().event("lagged").data(n.to_string()),
        })
    });
    let hello = stream::once(async { Ok::<_, Infallible>(SseEvent::default().event("ready").data("ok")) });
    let s: std::pin::Pin<Box<dyn Stream<Item = Result<SseEvent, Infallible>> + Send>> = Box::pin(hello.chain(live));
    Sse::new(s).keep_alive(KeepAlive::default()).into_response()
}

/// A counterparty's live risk, and what the policy would do with a small payment to it.
async fn screen(State(app): State<Shared>, headers: HeaderMap, Path(address): Path<Address>) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    let a = format!("{address:#x}");
    match app.screener.address(&a).await {
        Ok(scan) => {
            let cfg = &app.policy.cfg;
            let band = if scan.toxic_score >= cfg.refuse_at {
                "refuse"
            } else if scan.toxic_score >= cfg.ask_at {
                "ask"
            } else if scan.toxic_score >= cfg.cap_at {
                "cap"
            } else {
                "pay"
            };
            Json(json!({
                "address": a,
                "toxicScore": scan.toxic_score,
                "band": band,
                "reason": scan.reason(),
                "traits": scan.traits,
                "thresholds": { "refuseAt": cfg.refuse_at, "askAt": cfg.ask_at, "capAt": cfg.cap_at },
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("screening unavailable: {e:#}"), "address": a})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeBody {
    wallet: Address,
    seller: Address,
    /// Free text, or an Intercepta trait name (`wallet_drainer`, `phishing`, ...).
    #[serde(default)]
    reason: Option<String>,
}

/// Close a tab now. Always allowed, never needs a human: stopping payment is the safe direction.
async fn revoke(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<RevokeBody>) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    let reason = b.reason.filter(|r| !r.trim().is_empty()).unwrap_or_else(|| "closed by the operator".into());
    let code = match reason_code(&reason) {
        99 => 0, // free text, not a trait name
        c => c,
    };
    let entry = app.revoke(b.wallet, b.seller, reason, code, Actor::Operator, None).await;
    Json(entry).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreBody {
    wallet: Address,
    seller: Address,
}

/// Ask the wallet's human to reopen a closed tab. Nothing is restored until they approve,
/// with a fresh World ID verification, on their own device.
async fn restore(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<RestoreBody>) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    if !app.sessions.is_revoked(b.wallet, b.seller).await {
        let on_chain = match &app.guardian {
            Some(g) => g.standing(b.wallet, b.seller).await.map(|s| s.revoked).unwrap_or(false),
            None => false,
        };
        if !on_chain {
            return bad_request("that tab is not closed");
        }
    }
    let n = NewApproval {
        purpose: Purpose::Restore,
        wallet: b.wallet,
        seller: b.seller,
        amount: 0,
        reason: "Reopening a tab that was closed. Payments to this seller resume only if you approve.".into(),
        bound_digest: restore_digest(b.wallet, b.seller),
    };
    match app.request_approval(n).await {
        Ok(a) => {
            let mut v = serde_json::to_value(a.view()).unwrap_or_default();
            v["approvalUrl"] = json!(app.approval_link(&a));
            (StatusCode::ACCEPTED, Json(v)).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("{e:#}")}))).into_response(),
    }
}

/// Start "prove you are a unique human" for someone who wants a wallet. Nothing is bound yet.
async fn enroll(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    let n = NewApproval {
        purpose: Purpose::Enroll,
        wallet: Address::ZERO,
        seller: Address::ZERO,
        amount: 0,
        reason: "Verify you are a unique human to get your own Tab. One per person.".into(),
        bound_digest: format!("countersign.enroll:{:032x}", crate::approvals::random_u128()),
    };
    match app.request_approval(n).await {
        Ok(a) => {
            let mut v = serde_json::to_value(a.view()).unwrap_or_default();
            v["approvalUrl"] = json!(app.approval_link(&a));
            (StatusCode::ACCEPTED, Json(v)).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("{e:#}")}))).into_response(),
    }
}

/// Redeem an approved enrolment, once: the person's fingerprint, and a grant to bind a wallet.
async fn enroll_claim(State(app): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    match app.claim_enrollment(&id).await {
        Ok((human, grant)) => Json(json!({"human": human, "grant": grant})).into_response(),
        Err(crate::approvals::ApprovalError::NotYetApproved) => {
            (StatusCode::ACCEPTED, Json(json!({"status": "pending"}))).into_response()
        }
        Err(e) => (StatusCode::FORBIDDEN, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct BindBody {
    grant: String,
    wallet: Address,
}

/// Bind a newly deployed wallet to the person an enrolment verified. From then on only that
/// person's World ID can approve its payments or reopen its tabs.
async fn bind(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<BindBody>) -> Response {
    if let Err(r) = operator(&app, &headers) {
        return r;
    }
    match app.bind_with_grant(&b.grant, b.wallet).await {
        Ok(human) => Json(json!({"wallet": format!("{:#x}", b.wallet), "human": human})).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({"error": format!("{e:#}")}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::env::{CountersignerEnv, InterceptaEnv, NotifyEnv, PolicyEnv, ServiceEnv, WorldEnv};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "test-control-token-0123456789";
    const WALLET: &str = "0x1111111111111111111111111111111111111111";
    const SELLER: &str = "0x2222222222222222222222222222222222222222";

    /// No Intercepta key, no World, no chain: everything here must be decided without them,
    /// or fail closed.
    fn app(control_token: Option<&str>) -> Shared {
        let cfg = CountersignerEnv {
            bind: "127.0.0.1:0".into(),
            oracle_private_key: "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d".into(),
            intercepta: InterceptaEnv { api_key: String::new() },
            world: WorldEnv::Disabled,
            notify: NotifyEnv::Disabled,
            policy: PolicyEnv::new().unwrap(),
            service: ServiceEnv {
                rpc: None,
                control_token: control_token.map(String::from),
                state_dir: std::env::temp_dir().join(format!("cs-api-{}-{}", std::process::id(), rand_suffix())),
                approval_page_url: Some("https://tab.test".into()),
                push_wallets: None,
            },
        };
        Arc::new(App::from_env(&cfg).unwrap())
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    async fn call(app: Shared, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = router(app).oneshot(req).await.unwrap();
        let status = res.status();
        let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or_default())
    }

    fn post(path: &str, token: Option<&str>, body: serde_json::Value) -> Request<Body> {
        let mut r = Request::post(path).header("content-type", "application/json");
        if let Some(t) = token {
            r = r.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        r.body(Body::from(body.to_string())).unwrap()
    }

    fn countersign_body() -> serde_json::Value {
        json!({
            "channel": {
                "payer": WALLET,
                "receiver": SELLER,
                "receiverAuthorizer": "0x3333333333333333333333333333333333333333",
                "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            },
            "chainId": 8453,
            "ceiling": 10000,
        })
    }

    #[tokio::test]
    async fn the_operator_api_does_not_exist_without_a_token() {
        let (status, _) = call(app(None), post("/v1/revoke", Some(TOKEN), json!({"wallet": WALLET, "seller": SELLER}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_operator_api_rejects_a_wrong_token() {
        let (status, _) = call(app(Some(TOKEN)), post("/v1/revoke", Some("not-the-right-token-at-all"), json!({"wallet": WALLET, "seller": SELLER}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(app(Some(TOKEN)), post("/v1/revoke", None, json!({"wallet": WALLET, "seller": SELLER}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_unscreenable_payment_is_refused_and_journalled() {
        let a = app(Some(TOKEN));
        let (status, body) = call(a.clone(), post("/v1/countersign", None, countersign_body())).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body["reason"].as_str().unwrap().contains("failing closed"), "{body}");
        let e = a.journal.since(0).await;
        assert!(matches!(&e[0].event, Event::Screened { verdict, .. } if verdict == "refuse"));
    }

    #[tokio::test]
    async fn a_closed_tab_gets_no_signature_and_the_closure_is_journalled() {
        let a = app(Some(TOKEN));
        let (status, body) = call(
            a.clone(),
            post("/v1/revoke", Some(TOKEN), json!({"wallet": WALLET, "seller": SELLER, "reason": "wallet_drainer"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["kind"], "revoked");
        assert_eq!(body["by"], "operator");
        assert_eq!(body["reason_code"], 4, "a trait name maps to its on-chain code");
        assert!(body["chain_error"].as_str().unwrap().contains("no on-chain guardian"), "{body}");

        let (status, body) = call(a.clone(), post("/v1/countersign", None, countersign_body())).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body["reason"].as_str().unwrap().contains("closed"), "{body}");
    }

    #[tokio::test]
    async fn restoring_needs_a_closed_tab_and_a_human() {
        let a = app(Some(TOKEN));
        let body = json!({"wallet": WALLET, "seller": SELLER});
        let (status, _) = call(a.clone(), post("/v1/restore", Some(TOKEN), body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "an open tab has nothing to restore");

        call(a.clone(), post("/v1/revoke", Some(TOKEN), body.clone())).await;
        let (status, resp) = call(a.clone(), post("/v1/restore", Some(TOKEN), body)).await;
        // World is not configured here, so there is no human to ask: it must not restore anyway
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(resp["error"].as_str().unwrap().contains("World ID"), "{resp}");
        assert!(a.sessions.is_revoked(WALLET.parse().unwrap(), SELLER.parse().unwrap()).await);
    }

    #[tokio::test]
    async fn approvals_and_activity_are_readable_but_unknown_ids_are_not_found() {
        let a = app(Some(TOKEN));
        let (status, _) = call(a.clone(), Request::get("/v1/approvals/apr_nope").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(a.clone(), post("/v1/approve/poll", None, json!({"approvalId": "apr_nope"}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        call(a.clone(), post("/v1/revoke", Some(TOKEN), json!({"wallet": WALLET, "seller": SELLER}))).await;
        let req = Request::get("/v1/activity?after=0")
            .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(a.clone(), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entries"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn enrolment_is_operator_only_and_grants_are_single_use() {
        let a = app(Some(TOKEN));
        let (status, _) = call(a.clone(), post("/v1/enroll", None, json!({}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body) = call(a.clone(), post("/v1/enroll", Some(TOKEN), json!({}))).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "no World configured here: {body}");

        let (status, _) = call(a.clone(), post("/v1/enroll/apr_nope/claim", Some(TOKEN), json!({}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "an unknown enrolment yields nothing");
        let (status, body) = call(a.clone(), post("/v1/bind", Some(TOKEN), json!({"grant": "grant_forged", "wallet": WALLET}))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
    }

    #[test]
    fn token_comparison_needs_an_exact_match() {
        assert!(constant_time_eq(b"a-long-control-token", b"a-long-control-token"));
        assert!(!constant_time_eq(b"a-long-control-token", b"a-long-control-tokeN"));
        assert!(!constant_time_eq(b"short", b"a-long-control-token"));
        assert!(!constant_time_eq(b"", b"a-long-control-token"));
    }
}
