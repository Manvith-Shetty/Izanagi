//! The x402 endpoint and the loop that cashes vouchers in.

use crate::chain::{Chain, Rejected};
use crate::channels::{check_amounts, check_terms, claim_due, Channel, ClaimOutcome, ClaimReason, Reject};
use crate::env::SellerEnv;
use crate::screen::{PayerScreen, Standing, Verdict};
use alloy::primitives::{Address, B256};
use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use common::escrow::{claim_row, reason_name};
use common::x402::{
    self, decode_header, encode_header, errors, parse_amount, ChannelState, PaymentPayload,
    PaymentRequired, PaymentRequirements, RequirementsExtra, Resource, SettlementExtra,
    SettlementResponse, VoucherState,
};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};

pub const PAID_PATH: &str = "/v1/data";

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

pub struct App {
    pub env: SellerEnv,
    pub chain: Chain,
    pub screen: PayerScreen,
    /// One lock per channel: requests on a channel are processed one at a time (spec).
    channels: RwLock<HashMap<B256, Arc<Mutex<Channel>>>>,
}

pub type Shared = Arc<App>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimReport {
    pub channel_id: B256,
    pub payer: String,
    /// What made us claim now.
    pub trigger: ClaimReason,
    #[serde(flatten)]
    pub outcome: ClaimOutcome,
}

fn usdc(v: u128) -> String {
    format!("{}.{:06}", v / 1_000_000, v % 1_000_000)
}

fn header(name: &'static str, value: String) -> (HeaderName, HeaderValue) {
    (
        // normalises the spec's upper-case names; HTTP header names are case-insensitive
        HeaderName::from_bytes(name.as_bytes()).expect("a valid header name"),
        HeaderValue::from_str(&value).expect("base64 is a valid header value"),
    )
}

impl App {
    pub fn new(env: SellerEnv, chain: Chain, screen: PayerScreen) -> Self {
        Self { env, chain, screen, channels: RwLock::new(HashMap::new()) }
    }

    /// Our terms, as advertised in every 402.
    pub fn requirements(&self) -> PaymentRequirements {
        PaymentRequirements {
            scheme: x402::SCHEME.into(),
            network: x402::network(self.env.chain_id),
            amount: self.env.price.to_string(),
            asset: self.env.token,
            pay_to: self.env.receiver,
            max_timeout_seconds: 3600,
            extra: RequirementsExtra {
                receiver_authorizer: self.chain.authorizer(),
                withdraw_delay: self.env.withdraw_delay,
                name: self.env.token_name.clone(),
                version: self.env.token_version.clone(),
                min_deposit: None,
                channel_state: None,
                voucher_state: None,
            },
        }
    }

    /// A 402 carrying our terms, optionally corrective (the channel as we see it).
    fn payment_required(
        &self,
        error: &str,
        message: Option<&str>,
        corrective: Option<(ChannelState, Option<VoucherState>)>,
    ) -> Response {
        let mut req = self.requirements();
        if let Some((cs, vs)) = corrective {
            req.extra.channel_state = Some(cs);
            req.extra.voucher_state = vs;
        }
        let body = PaymentRequired {
            x402_version: x402::VERSION,
            error: Some(error.to_string()),
            resource: Some(Resource {
                url: format!("{}{PAID_PATH}", self.env.public_url),
                description: "Countersign demo data feed, 0.01 USDC per call".into(),
                mime_type: "application/json".into(),
            }),
            accepts: vec![req],
        };
        let h = header(x402::HEADER_REQUIRED, encode_header(&body).expect("serializable"));
        let mut json = serde_json::to_value(&body).expect("serializable");
        if let Some(m) = message {
            json["message"] = json!(m);
        }
        (StatusCode::PAYMENT_REQUIRED, [h], Json(json)).into_response()
    }

    fn rejected(&self, r: Reject, corrective: Option<(ChannelState, Option<VoucherState>)>) -> Response {
        tracing::info!(code = r.code, "payment rejected: {}", r.message);
        self.payment_required(r.code, Some(&r.message), corrective)
    }

    fn settlement(&self, payer: Address, success: bool, error: Option<String>, extra: Option<SettlementExtra>) -> (HeaderName, HeaderValue) {
        let r = SettlementResponse {
            success,
            error_reason: error,
            transaction: String::new(),
            network: x402::network(self.env.chain_id),
            payer: format!("{payer:#x}"),
            amount: String::new(),
            extra,
        };
        header(x402::HEADER_RESPONSE, encode_header(&r).expect("serializable"))
    }

    /// Paying cannot fix this, so it is not a 402.
    fn refused_payer(&self, payer: Address, v: &Verdict) -> Response {
        tracing::warn!(payer = %payer, score = v.toxic_score, "refusing to serve payer: {}", v.reason);
        let h = self.settlement(payer, false, Some("payer_refused".into()), None);
        let body = json!({ "error": "payer_refused", "reason": v.reason, "toxicScore": v.toxic_score });
        (StatusCode::FORBIDDEN, [h], Json(body)).into_response()
    }

    fn unavailable(&self, code: &str, e: impl std::fmt::Display) -> Response {
        // `{:#}` prints the whole cause chain ("escrow.channels: ... 429 Too Many Requests"),
        // not just the outermost context, so an outage says what actually failed.
        tracing::error!("{code}: {e:#}");
        let body = json!({ "error": code, "reason": format!("{e:#}") });
        (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
    }

    async fn channel_lock(&self, id: B256, make: impl FnOnce() -> Channel) -> Arc<Mutex<Channel>> {
        if let Some(c) = self.channels.read().await.get(&id) {
            return c.clone();
        }
        self.channels.write().await.entry(id).or_insert_with(|| Arc::new(Mutex::new(make()))).clone()
    }

    /// Accept one paid request. Order matters: everything that can be checked without the
    /// chain is checked first, and state changes only after the response is produced.
    pub async fn pay(&self, headers: &HeaderMap) -> Response {
        let Some(raw) = headers.get(x402::HEADER_SIGNATURE) else {
            return self.payment_required(&format!("{} header is required", x402::HEADER_SIGNATURE), None, None);
        };
        let Some(p) = raw.to_str().ok().and_then(|s| decode_header::<PaymentPayload>(s).ok()) else {
            return self.rejected(
                Reject { code: errors::VOUCHER_PAYLOAD, message: "not a base64 x402 PaymentPayload".into() },
                None,
            );
        };

        let chain_id = self.env.chain_id;
        let cfg = match check_terms(&p, &self.requirements(), chain_id) {
            Ok(c) => c,
            Err(r) => return self.rejected(r, None),
        };
        let Ok(max) = parse_amount(&p.payload.voucher.max_claimable_amount) else {
            return self.rejected(
                Reject { code: errors::VOUCHER_PAYLOAD, message: "maxClaimableAmount is not an integer".into() },
                None,
            );
        };

        // who is paying us, and how much credit do they get?
        let verdict = self.screen.verdict(cfg.payer).await;
        if verdict.standing == Standing::Refused {
            return self.refused_payer(cfg.payer, &verdict);
        }

        let id = p.payload.voucher.channel_id;
        let onchain = match self.chain.channel(id).await {
            Ok(c) => c,
            Err(e) => return self.unavailable(errors::RPC_READ_FAILED, e),
        };
        let lock = self.channel_lock(id, || Channel::new(cfg.clone(), id, &onchain, now())).await;
        let mut ch = lock.lock().await;
        ch.sync(&onchain, now());

        if let Err(r) = check_amounts(max, ch.charged, self.env.price, &onchain) {
            let corrective = (r.code == errors::CUMULATIVE_MISMATCH).then(|| (ch.channel_state(), ch.voucher_state()));
            return self.rejected(r, corrective);
        }

        // ask the payer's wallet the question the escrow will ask at claim time
        let sig = p.payload.voucher.signature.clone();
        let digest = common::voucher_digest(id, max, chain_id);
        match self.chain.voucher_valid(&cfg, digest, &sig).await {
            Ok(true) => {}
            Ok(false) => {
                return self.rejected(
                    Reject {
                        code: errors::VOUCHER_SIGNATURE,
                        message: "the payer's wallet does not accept this voucher. For a Countersign wallet that means \
                                  no valid countersignature, a lapsed attestation, this seller revoked, or the wallet paused"
                            .into(),
                    },
                    None,
                )
            }
            Err(e) => return self.unavailable(errors::RPC_READ_FAILED, e),
        }

        // serve, then commit
        let served = ch.requests + 1;
        let body = json!({
            "data": format!("paid response #{served} on this channel"),
            "servedAt": now(),
        });
        ch.commit(self.env.price, max, sig, now());
        ch.standing = verdict.standing;
        ch.payer_score = verdict.toxic_score;
        tracing::info!(
            channel = %id, payer = %cfg.payer, standing = ?verdict.standing,
            "served #{served}: charged {} USDC, {} unclaimed",
            usdc(ch.charged), usdc(ch.unclaimed())
        );

        let h = self.settlement(
            cfg.payer,
            true,
            None,
            Some(SettlementExtra {
                charged_amount: Some(self.env.price.to_string()),
                channel_state: Some(ch.channel_state()),
            }),
        );
        (StatusCode::OK, [h], Json(body)).into_response()
    }

    /// One pass over every channel. `force` cashes in anything unclaimed (an operator asked).
    pub async fn claim_round(&self, force: bool) -> Vec<ClaimReport> {
        let locks: Vec<_> = self.channels.read().await.values().cloned().collect();
        let mut reports = Vec::new();
        for lock in locks {
            let mut ch = lock.lock().await;
            // refresh first: a landed claim or a started withdrawal changes the answer
            match self.chain.channel(ch.channel_id).await {
                Ok(c) => ch.sync(&c, now()),
                Err(e) => {
                    tracing::warn!(channel = %ch.channel_id, "could not refresh channel: {e:#}");
                    continue;
                }
            }
            let reason = match claim_due(&ch, now(), &self.env.claim) {
                Some(r) => r,
                None if force && ch.unclaimed() > 0 => ClaimReason::Manual,
                None => continue,
            };
            let outcome = self.cash_in(&mut ch).await;
            ch.last_claim = Some(outcome.clone());
            reports.push(ClaimReport { channel_id: ch.channel_id, payer: ch.payer.clone(), trigger: reason, outcome });
        }

        if reports.iter().any(|r| matches!(r.outcome, ClaimOutcome::Claimed { .. })) {
            match self.chain.settle(self.env.receiver, self.env.token).await {
                Ok(tx) => tracing::info!("settled claimed funds to {:#x}: {tx}", self.env.receiver),
                // claimed value stays in the escrow, credited to us; the next round retries
                Err(e) => tracing::warn!("settle failed, will retry after the next claim: {e:#}"),
            }
        }
        reports
    }

    async fn cash_in(&self, ch: &mut Channel) -> ClaimOutcome {
        let amount = ch.unclaimed();
        let at = now();
        let rows = vec![claim_row(&ch.cfg, ch.signed_max, ch.signature.clone(), ch.charged)];
        match self.chain.simulate_claim(rows.clone()).await {
            Err(e) => ClaimOutcome::Failed { error: format!("{e:#}"), at },
            Ok(Err(Rejected(msg))) => {
                let reason = self.why_rejected(ch, &msg).await;
                tracing::error!(
                    channel = %ch.channel_id, payer = %ch.payer,
                    "CLAIM REJECTED by the escrow: {} USDC we served for is unclaimable. {reason}",
                    usdc(amount)
                );
                ClaimOutcome::Rejected { reason, unclaimable: amount, at }
            }
            Ok(Ok(())) => match self.chain.claim(rows).await {
                Ok(tx) => {
                    tracing::info!(channel = %ch.channel_id, "claimed {} USDC: {tx}", usdc(amount));
                    let total = ch.charged;
                    ch.chain.total_claimed = total;
                    ch.unclaimed_since = None;
                    ClaimOutcome::Claimed { tx: format!("{tx:#x}"), amount, at }
                }
                Err(e) => ClaimOutcome::Failed { error: format!("{e:#}"), at },
            },
        }
    }

    /// Turn "execution reverted" into the actual cause, when the payer will tell us.
    async fn why_rejected(&self, ch: &Channel, revert: &str) -> String {
        if ch.attestation_expiry.is_none() {
            return format!("the payer's wallet no longer accepts the voucher ({revert})");
        }
        match self.chain.countersign_status(ch.cfg.payer, self.env.receiver).await {
            Some(s) if s.revoked => format!(
                "the payer's Countersign wallet revoked this seller at {} (reason: {}) - after we had served the requests",
                s.revoked_at,
                reason_name(s.reason)
            ),
            Some(s) if s.paused => "the payer's Countersign wallet is paused".to_string(),
            _ => match ch.attestation_expiry {
                Some(exp) if now() > exp => format!(
                    "the Countersign attestation lapsed at {exp} and was not renewed - we claimed too late"
                ),
                _ => format!("the payer's wallet no longer accepts the voucher ({revert})"),
            },
        }
    }

    pub async fn channels(&self) -> Vec<Channel> {
        let locks: Vec<_> = self.channels.read().await.values().cloned().collect();
        let mut out = Vec::with_capacity(locks.len());
        for l in locks {
            out.push(l.lock().await.clone());
        }
        out
    }
}

/* ---------------------------------- routes ---------------------------------- */

async fn paid(State(app): State<Shared>, headers: HeaderMap) -> Response {
    app.pay(&headers).await
}

async fn health(State(app): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "receiver": format!("{:#x}", app.env.receiver),
        "receiverAuthorizer": format!("{:#x}", app.chain.authorizer()),
        "price": usdc(app.env.price),
        "network": x402::network(app.env.chain_id),
        "payerScreening": !app.env.intercepta_api_key.trim().is_empty(),
    }))
}

async fn list_channels(State(app): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({ "channels": app.channels().await }))
}

async fn admin_claim(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let expected = app.env.admin_token.as_deref().map(|t| format!("Bearer {t}"));
    let given = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    if expected.is_none() || given != expected.as_deref() {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(json!({ "claims": app.claim_round(true).await })).into_response()
}

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(PAID_PATH, get(paid))
        .route("/v1/channels", get(list_channels))
        .route("/admin/claim", post(admin_claim))
        .with_state(app)
}

/// Runs forever: cash in whatever is due.
pub async fn claim_loop(app: Shared) {
    let mut ticker = tokio::time::interval(Duration::from_secs(app.env.claim.tick_secs.max(1)));
    loop {
        ticker.tick().await;
        for r in app.claim_round(false).await {
            tracing::debug!(?r, "claim round");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{ClaimConfig, ScreenConfig};
    use alloy::primitives::{address, Bytes};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use common::intercepta::Intercepta;
    use common::x402::{ChannelConfigJson, SchemePayload, VoucherJson};
    use common::ChannelConfig;
    use tower::ServiceExt;

    /// An app with no Intercepta key and an RPC nobody listens on: every request here must be
    /// answered before the chain is consulted, or fail closed.
    fn app() -> Shared {
        let env = SellerEnv {
            bind: "127.0.0.1:0".into(),
            public_url: "http://seller.test".into(),
            receiver: address!("2222222222222222222222222222222222222222"),
            receiver_authorizer_key: "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba".into(),
            price: 10_000,
            token: common::USDC_BASE,
            token_name: "USD Coin".into(),
            token_version: "2".into(),
            withdraw_delay: 900,
            chain_id: 8453,
            rpc: "http://127.0.0.1:9".into(),
            intercepta_api_key: String::new(),
            screen: ScreenConfig { refuse_at: 60.0, careful_at: 30.0, cache_secs: 60 },
            claim: ClaimConfig { tick_secs: 10, margin_secs: 30, max_unclaimed: 1_000_000, max_age_secs: 3600 },
            admin_token: Some("s3cret".into()),
        };
        let chain = Chain::new(&env.rpc, &env.receiver_authorizer_key).unwrap();
        let screen = PayerScreen::new(Intercepta::new(String::new()), env.screen.clone());
        Arc::new(App::new(env, chain, screen))
    }

    fn voucher(app: &App, kind: &str) -> String {
        let r = app.requirements();
        let cfg = ChannelConfig::countersign(
            address!("1111111111111111111111111111111111111111"),
            r.pay_to,
            r.extra.receiver_authorizer,
            r.asset,
            900,
            B256::ZERO,
        );
        encode_header(&PaymentPayload {
            x402_version: 2,
            payload: SchemePayload {
                kind: kind.into(),
                channel_config: ChannelConfigJson::from(&cfg),
                voucher: VoucherJson {
                    channel_id: common::channel_id(&cfg, 8453),
                    max_claimable_amount: "10000".into(),
                    signature: Bytes::from(vec![0u8; 65]),
                },
                amount: None,
                deposit: None,
            },
            accepted: r,
        })
        .unwrap()
    }

    async fn get(app: Shared, payment: Option<&str>) -> (StatusCode, HeaderMap, serde_json::Value) {
        let mut req = Request::get(PAID_PATH);
        if let Some(p) = payment {
            req = req.header(x402::HEADER_SIGNATURE, p);
        }
        let res = router(app).oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let (parts, body) = res.into_parts();
        let body = to_bytes(body, 1 << 16).await.unwrap();
        (parts.status, parts.headers, serde_json::from_slice(&body).unwrap_or_default())
    }

    #[tokio::test]
    async fn an_unpaid_request_gets_our_terms() {
        let a = app();
        let (status, headers, _) = get(a.clone(), None).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);

        let pr: PaymentRequired =
            decode_header(headers.get(x402::HEADER_REQUIRED).unwrap().to_str().unwrap()).unwrap();
        assert_eq!(pr.x402_version, 2);
        let terms = &pr.accepts[0];
        assert_eq!(terms.scheme, "batch-settlement");
        assert_eq!(terms.network, "eip155:8453");
        assert_eq!(terms.amount, "10000");
        assert_eq!(terms.pay_to, a.env.receiver);
        assert_eq!(terms.extra.receiver_authorizer, a.chain.authorizer());
        assert_eq!(pr.resource.unwrap().url, "http://seller.test/v1/data");
    }

    #[tokio::test]
    async fn a_malformed_payment_is_a_402_with_the_spec_code() {
        let (status, _, body) = get(app(), Some("definitely not base64 json")).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body["error"], errors::VOUCHER_PAYLOAD);
    }

    #[tokio::test]
    async fn deposits_are_turned_away_with_directions() {
        let a = app();
        let (status, _, body) = get(a.clone(), Some(&voucher(&a, "deposit"))).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body["error"], errors::PAYLOAD_TYPE);
        assert!(body["message"].as_str().unwrap().contains("openChannel"));
    }

    #[tokio::test]
    async fn an_unscreenable_payer_is_refused_not_served() {
        // no Intercepta key: the seller must not fall through to serving
        let a = app();
        let (status, headers, body) = get(a.clone(), Some(&voucher(&a, "voucher"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "payer_refused");
        let sr: SettlementResponse =
            decode_header(headers.get(x402::HEADER_RESPONSE).unwrap().to_str().unwrap()).unwrap();
        assert!(!sr.success);
        assert!(a.channels().await.is_empty(), "a refused payer leaves no channel state");
    }

    #[tokio::test]
    async fn admin_claim_is_invisible_without_the_token() {
        let res = router(app())
            .oneshot(Request::post("/admin/claim").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let res = router(app())
            .oneshot(
                Request::post("/admin/claim")
                    .header(AUTHORIZATION, "Bearer s3cret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "no channels yet, so an empty claim round");
    }

    /* ------------------------------------------------------------------------------------
       Against a fork of Base mainnet, with Coinbase's real escrow and real USDC:

         anvil --fork-url https://mainnet.base.org
         source <(scripts/fork-setup.sh)
         cargo test -p seller -- --ignored --nocapture
       ------------------------------------------------------------------------------------ */

    alloy::sol! {
        #[sol(rpc)]
        interface ICountersignOwner {
            function revoke(address seller, uint32 reason) external;
            function restore(address seller) external;
        }
        #[sol(rpc)]
        interface IERC20Balance {
            function balanceOf(address) external view returns (uint256);
        }
    }

    fn var(k: &str) -> String {
        std::env::var(k).unwrap_or_else(|_| panic!("{k} unset: run scripts/fork-setup.sh first"))
    }

    /// A voucher exactly as the agent builds one: the countersigner attests
    /// (digest, seller, expiry), the agent signs the digest, both go in one blob.
    fn countersigned(cfg: &ChannelConfig, max: u128, oracle: &str, agent: &str, expiry: u64) -> Bytes {
        use alloy::signers::{local::PrivateKeySigner, SignerSync};
        let digest = common::voucher_digest(common::channel_id(cfg, 8453), max, 8453);
        let o: PrivateKeySigner = oracle.trim_start_matches("0x").parse().unwrap();
        let a: PrivateKeySigner = agent.trim_start_matches("0x").parse().unwrap();
        let osig = o.sign_hash_sync(&common::attestation_hash(digest, cfg.receiver, expiry)).unwrap();
        let asig = a.sign_hash_sync(&digest).unwrap();
        Bytes::from(common::encode_signature_blob(&asig.as_bytes(), &osig.as_bytes(), cfg.receiver, expiry))
    }

    fn pay_header(app: &App, cfg: &ChannelConfig, max: u128, sig: Bytes) -> String {
        encode_header(&PaymentPayload {
            x402_version: 2,
            accepted: app.requirements(),
            payload: SchemePayload {
                kind: "voucher".into(),
                channel_config: ChannelConfigJson::from(cfg),
                voucher: VoucherJson {
                    channel_id: common::channel_id(cfg, 8453),
                    max_claimable_amount: max.to_string(),
                    signature: sig,
                },
                amount: None,
                deposit: None,
            },
        })
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "needs a Base fork prepared by scripts/fork-setup.sh"]
    async fn fork_serve_revoke_reject_restore_claim() {
        use alloy::network::EthereumWallet;
        use alloy::providers::ProviderBuilder;
        use alloy::signers::local::PrivateKeySigner;

        let rpc = var("BASE_RPC");
        let wallet: Address = var("COUNTERSIGN_WALLET").parse().unwrap();
        let (agent, oracle) = (var("AGENT_PRIVATE_KEY"), var("ORACLE_PRIVATE_KEY"));

        let mut env = app().env.clone();
        env.rpc = rpc.clone();
        env.receiver = var("SELLER_RECEIVER").parse().unwrap();
        env.receiver_authorizer_key = var("SELLER_AUTHORIZER_KEY");
        let chain = Chain::new(&env.rpc, &env.receiver_authorizer_key).unwrap();
        let screen = PayerScreen::new(Intercepta::new(String::new()), env.screen.clone());
        // no Intercepta key in CI: stand in a clean verdict for the payer, and only the payer
        screen
            .seed(wallet, Verdict { standing: Standing::Trusted, toxic_score: 0.0, reason: "seeded".into() })
            .await;
        let a = Arc::new(App::new(env, chain, screen));
        let price = a.env.price;

        let cfg = ChannelConfig::countersign(
            wallet, a.env.receiver, a.chain.authorizer(), common::USDC_BASE, 900, B256::ZERO,
        );
        let expiry = now() + 120;

        // 1. two paid requests, each a cumulative voucher one price higher
        for n in 1..=2u128 {
            let h = pay_header(&a, &cfg, n * price, countersigned(&cfg, n * price, &oracle, &agent, expiry));
            let (status, headers, body) = get(a.clone(), Some(&h)).await;
            assert_eq!(status, StatusCode::OK, "request {n}: {body}");
            let sr: SettlementResponse =
                decode_header(headers.get(x402::HEADER_RESPONSE).unwrap().to_str().unwrap()).unwrap();
            let cs = sr.extra.unwrap().channel_state.unwrap();
            assert_eq!(cs.charged_cumulative_amount.unwrap(), (n * price).to_string());
            println!("served #{n}: {body}");
        }

        // 2. a voucher that skips ahead gets a corrective 402 carrying our view of the channel
        let h = pay_header(&a, &cfg, 5 * price, countersigned(&cfg, 5 * price, &oracle, &agent, expiry));
        let (status, _, body) = get(a.clone(), Some(&h)).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body["error"], errors::CUMULATIVE_MISMATCH);
        assert_eq!(body["accepts"][0]["extra"]["channelState"]["chargedCumulativeAmount"], (2 * price).to_string());

        // 3. the agent alone, with no countersignature: the payer's own wallet says no
        let forged = countersigned(&cfg, 3 * price, &agent, &agent, expiry);
        let (status, _, body) = get(a.clone(), Some(&pay_header(&a, &cfg, 3 * price, forged))).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body["error"], errors::VOUCHER_SIGNATURE);

        // 4. AFTER we served, the owner revokes us on chain
        let owner: PrivateKeySigner = var("OWNER_KEY").trim_start_matches("0x").parse().unwrap();
        let owner_rpc = ProviderBuilder::new().wallet(EthereumWallet::from(owner)).connect_http(rpc.parse().unwrap());
        let cs = ICountersignOwner::new(wallet, &owner_rpc);
        cs.revoke(a.env.receiver, 4).send().await.unwrap().get_receipt().await.unwrap();

        // 5. the claim for requests we already served is refused by Coinbase's escrow
        let reports = a.claim_round(true).await;
        println!("after revoke: {}", serde_json::to_string_pretty(&reports).unwrap());
        match &reports[0].outcome {
            ClaimOutcome::Rejected { reason, unclaimable, .. } => {
                assert_eq!(*unclaimable, 2 * price);
                assert!(reason.contains("revoked") && reason.contains("wallet_drainer"), "{reason}");
            }
            o => panic!("expected the escrow to reject the claim, got {o:?}"),
        }

        // 6. and we are cut off: the next voucher is refused at the door
        let h = pay_header(&a, &cfg, 3 * price, countersigned(&cfg, 3 * price, &oracle, &agent, expiry));
        let (status, _, body) = get(a.clone(), Some(&h)).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body["error"], errors::VOUCHER_SIGNATURE);

        // 7. restored (a false positive): the same vouchers claim, and settle to our wallet
        cs.restore(a.env.receiver).send().await.unwrap().get_receipt().await.unwrap();
        let usdc = IERC20Balance::new(common::USDC_BASE, &owner_rpc);
        let before = usdc.balanceOf(a.env.receiver).call().await.unwrap();
        let reports = a.claim_round(true).await;
        println!("after restore: {}", serde_json::to_string_pretty(&reports).unwrap());
        assert!(matches!(reports[0].outcome, ClaimOutcome::Claimed { amount, .. } if amount == 2 * price));
        let on = a.chain.channel(common::channel_id(&cfg, 8453)).await.unwrap();
        assert_eq!(on.total_claimed, 2 * price);
        let after = usdc.balanceOf(a.env.receiver).call().await.unwrap();
        assert_eq!(after - before, alloy::primitives::U256::from(2 * price), "settled to the receiver");
    }
}
