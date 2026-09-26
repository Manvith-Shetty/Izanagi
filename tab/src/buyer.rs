//! Buying from x402 sellers on a person's behalf.
//!
//! One purchase: read the seller's 402, ask the countersigner (which screens the seller and
//! may ask the person first), open a tab from the person's own trial funds if there is none
//! yet, pay with the countersigned cumulative voucher, and keep the receipt. Nothing here
//! can authorise a payment: without the countersigner's signature the voucher is worthless.

use crate::brain::Brain;
use crate::chain::Chain;
use crate::feed::Feed;
use crate::store::{now, Account, Receipt, Store, TabRecord};
use countersign_agent::client::{Authorization, CountersignClient, CountersignResponse};
use countersign_agent::x402::{channel_for, payment_header, terms_from_402, Offer};
use alloy::primitives::{Address, B256};
use anyhow::Result;
use common::x402::{self, decode_header, errors, parse_amount, SettlementResponse};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

/// How much of a paid response we keep and hand back. Enough for data, not for a movie.
const BODY_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// Paid, and the seller delivered.
    Paid(Paid),
    /// A person must approve first. Show them `approval_url`.
    NeedsApproval(NeedsApproval),
    /// The countersigner (or the seller) said no. `reason` says why.
    Refused(Refused),
    /// This is not something Tab can pay for (not x402 batch-settlement on our chain, no funds).
    Unpayable { url: String, reason: String },
    /// The resource did not ask for payment.
    Free { url: String, status: u16, body: String },
    /// The person said no, or did not answer in time.
    Denied { approval_id: String, reason: String },
    /// Still waiting on the person.
    StillPending { approval_id: String, approval_url: String, user_code: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paid {
    pub url: String,
    pub seller: Address,
    pub service: Option<String>,
    pub price: u128,
    /// What this seller has charged this wallet in total.
    pub cumulative: u128,
    /// Signed but not yet claimed by the seller: still stoppable.
    pub stoppable: u128,
    /// When the newest voucher's countersignature lapses.
    pub expiry: u64,
    pub channel_id: B256,
    pub verdict: String,
    pub reason: String,
    pub toxic_score: f64,
    pub status: u16,
    pub content_type: Option<String>,
    /// The response, parsed when it is JSON.
    pub data: Value,
    /// Set when this purchase had to open (or top up) the tab first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened: Option<Opened>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub tx: String,
    pub deposit: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedsApproval {
    pub url: String,
    pub approval_id: String,
    /// Tab's page that shows the person what they are approving.
    pub approval_url: String,
    /// World's own page, with the code pre-filled where possible.
    pub world_url: String,
    pub user_code: String,
    pub expires_in: u64,
    pub seller: Address,
    pub service: Option<String>,
    /// The tab limit the person is asked to approve.
    pub limit: u128,
    pub requested: u128,
    pub approved_so_far: u128,
    pub reason: String,
    pub toxic_score: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refused {
    pub url: String,
    pub seller: Option<Address>,
    pub verdict: String,
    pub reason: String,
    pub toxic_score: f64,
}

#[derive(Clone)]
struct Pending {
    account: String,
    url: String,
}

pub struct Buyer {
    http: reqwest::Client,
    countersigner: CountersignClient,
    brain: Brain,
    chain: Arc<Chain>,
    store: Arc<Store>,
    feed: Arc<Feed>,
    chain_id: u64,
    tab_deposit: u128,
    max_price: u128,
    locks: Mutex<HashMap<B256, Arc<Mutex<()>>>>,
    pending: RwLock<HashMap<String, Pending>>,
}

fn usdc(v: u128) -> String {
    format!("{}.{:06}", v / 1_000_000, v % 1_000_000)
}

/// The seller's name for itself, from the 402's `resource`, if it offers one.
fn service_name(header: &str) -> Option<String> {
    let v: Value = decode_header(header).ok()?;
    let r = &v["resource"];
    if let Some(n) = r["serviceName"].as_str() {
        return Some(n.chars().take(48).collect());
    }
    Some(short_service(r["description"].as_str()?))
}

/// A seller's short name from its description:
/// "Tab demo shop: live Bitcoin price..." -> "Tab demo shop"; "Latest block - tip" -> "Latest block".
pub fn short_service(d: &str) -> String {
    let short = [":", " - ", " — ", ". "].iter().filter_map(|sep| d.split_once(sep).map(|(a, _)| a)).min_by_key(|a| a.len()).unwrap_or(d);
    short.trim().chars().take(48).collect()
}

impl Buyer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        countersigner_url: &str,
        agent_key: &str,
        brain: Brain,
        chain: Arc<Chain>,
        store: Arc<Store>,
        feed: Arc<Feed>,
        chain_id: u64,
        tab_deposit: u128,
        max_price: u128,
    ) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?,
            countersigner: CountersignClient::new(countersigner_url, agent_key)?,
            brain,
            chain,
            store,
            feed,
            chain_id,
            tab_deposit,
            max_price,
            locks: Mutex::new(HashMap::new()),
            pending: RwLock::new(HashMap::new()),
        })
    }

    async fn lock(&self, id: B256) -> Arc<Mutex<()>> {
        self.locks.lock().await.entry(id).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    }

    /// Buy one call to `url` for `account`. `approval_id` redeems a person's approval.
    pub async fn buy(&self, account: &Account, url: &str, approval_id: Option<&str>) -> Outcome {
        match self.try_buy(account, url, approval_id).await {
            Ok(o) => o,
            Err(e) => Outcome::Unpayable { url: url.into(), reason: format!("{e:#}") },
        }
    }

    async fn try_buy(&self, account: &Account, url: &str, approval_id: Option<&str>) -> Result<Outcome> {
        // 1. the seller's terms
        let first = self.http.get(url).send().await?;
        if first.status() != reqwest::StatusCode::PAYMENT_REQUIRED {
            let status = first.status().as_u16();
            let body: String = first.text().await.unwrap_or_default().chars().take(BODY_LIMIT).collect();
            return Ok(Outcome::Free { url: url.into(), status, body });
        }
        let Some(header) = first.headers().get(x402::HEADER_REQUIRED).and_then(|h| h.to_str().ok()).map(String::from) else {
            return Ok(Outcome::Unpayable { url: url.into(), reason: "the seller answered 402 without its terms".into() });
        };
        let offer = match terms_from_402(&header, self.chain_id) {
            Ok(o) => o,
            Err(e) => {
                return Ok(Outcome::Unpayable {
                    url: url.into(),
                    reason: format!("{e:#}. Tab pays through batch-settlement channels, the kind of x402 payment that can still be stopped after it is signed"),
                })
            }
        };
        let service = service_name(&header);
        let price = parse_amount(&offer.terms.amount)?;
        if price > self.max_price {
            return Ok(Outcome::Refused(Refused {
                url: url.into(),
                seller: Some(offer.terms.pay_to),
                verdict: "refuse".into(),
                reason: format!("this seller asks {} USDC per call; this agent pays at most {}", usdc(price), usdc(self.max_price)),
                toxic_score: -1.0,
            }));
        }

        let cfg = channel_for(&offer.terms, account.wallet, B256::ZERO);
        let id = common::channel_id(&cfg, self.chain_id);
        let lock = self.lock(id).await;
        let _held = lock.lock().await;

        let mut retried = false;
        loop {
            let onchain = self.chain.tab(id).await?;
            let mut rec = match self.store.tab(id).await {
                Some(r) => r,
                None => TabRecord {
                    channel_id: id,
                    account: account.id.clone(),
                    seller: offer.terms.pay_to,
                    receiver_authorizer: offer.terms.extra.receiver_authorizer,
                    token: offer.terms.asset,
                    withdraw_delay: offer.terms.extra.withdraw_delay,
                    salt: B256::ZERO,
                    service: service.clone(),
                    price,
                    charged: 0,
                    signed: 0,
                    expiry: 0,
                    requests: 0,
                    deposited: 0,
                    opened_at: now(),
                    open_tx: None,
                    claimed: 0,
                },
            };
            rec.claimed = onchain.total_claimed;
            let charged = rec.charged.max(onchain.total_claimed);
            let ceiling = charged + price;

            // 2. the decision is the countersigner's
            let (resp, voucher) = match self.countersigner.request(&cfg, self.chain_id, ceiling, approval_id).await? {
                Authorization::Refused(r) => {
                    self.feed_refused(account, url, &offer, &r).await;
                    return Ok(Outcome::Refused(refused(url, &offer, &r)));
                }
                Authorization::NeedsHuman(r) => return Ok(self.needs_approval(account, url, &offer, &service, r).await),
                Authorization::Signed(r, v) => (r, v),
            };
            if voucher.ceiling < ceiling {
                return Ok(Outcome::Refused(Refused {
                    url: url.into(),
                    seller: Some(offer.terms.pay_to),
                    verdict: "cap".into(),
                    reason: format!(
                        "risk caps this tab at {} USDC, below the {} USDC the next call needs: {}",
                        usdc(voucher.ceiling), usdc(ceiling), resp.reason
                    ),
                    toxic_score: resp.toxic_score,
                }));
            }

            // 3. a tab needs escrowed funds before the seller will serve against it
            let mut opened = None;
            if onchain.balance < ceiling {
                let need = ceiling - onchain.balance;
                let deposit = self.tab_deposit.max(need);
                let w = self.chain.wallet(account.wallet).await?;
                let available = w.usdc.min(w.allowance);
                if available < need {
                    return Ok(Outcome::Unpayable {
                        url: url.into(),
                        reason: format!("your wallet has {} USDC it can put into tabs; this needs {}", usdc(available), usdc(need)),
                    });
                }
                let deposit = deposit.min(available);
                let tx = self.chain.open_tab(&cfg, deposit).await?;
                rec.deposited += deposit;
                rec.open_tx.get_or_insert_with(|| format!("{tx:#x}"));
                self.feed
                    .tab(
                        if onchain.balance == 0 { "tab_opened" } else { "tab_topped_up" },
                        json!({"wallet": account.wallet, "seller": offer.terms.pay_to, "service": service,
                               "channelId": id, "deposit": deposit, "tx": format!("{tx:#x}")}),
                    )
                    .await;
                opened = Some(Opened { tx: format!("{tx:#x}"), deposit });
            }

            // 4. pay
            let res = self
                .http
                .get(url)
                .header(x402::HEADER_SIGNATURE, payment_header(&offer, &cfg, &voucher)?)
                .send()
                .await?;
            let status = res.status();
            let receipt = res.headers().get(x402::HEADER_RESPONSE).and_then(|h| h.to_str().ok()).map(String::from);
            let content_type = res.headers().get("content-type").and_then(|h| h.to_str().ok()).map(String::from);
            let text: String = res.text().await.unwrap_or_default().chars().take(BODY_LIMIT).collect();
            let data: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));

            if status.is_success() {
                let charged_now = receipt
                    .as_deref()
                    .and_then(|h| decode_header::<SettlementResponse>(h).ok())
                    .and_then(|r| r.extra.and_then(|e| e.charged_amount))
                    .and_then(|c| parse_amount(&c).ok())
                    .filter(|c| *c <= price)
                    // a seller that does not say what it charged is taken at its price
                    .unwrap_or(price);
                rec.charged = charged + charged_now;
                rec.signed = rec.signed.max(ceiling);
                rec.expiry = voucher.expiry;
                rec.requests += 1;
                rec.price = price;
                if rec.service.is_none() {
                    rec.service = service.clone();
                }
                let stoppable = rec.charged.saturating_sub(rec.claimed);
                self.store.put_tab(rec.clone()).await?;
                self.store
                    .add_receipt(Receipt {
                        at: now(),
                        account: account.id.clone(),
                        seller: offer.terms.pay_to,
                        service: rec.service.clone(),
                        url: url.into(),
                        price: charged_now,
                        cumulative: rec.charged,
                        verdict: resp.action().into(),
                        toxic_score: resp.toxic_score,
                    })
                    .await?;
                self.feed
                    .tab(
                        "purchase",
                        json!({"wallet": account.wallet, "seller": offer.terms.pay_to, "service": rec.service,
                               "url": url, "price": charged_now, "cumulative": rec.charged, "stoppable": stoppable,
                               "expiry": voucher.expiry, "channelId": id}),
                    )
                    .await;
                return Ok(Outcome::Paid(Paid {
                    url: url.into(),
                    seller: offer.terms.pay_to,
                    service: rec.service,
                    price: charged_now,
                    cumulative: rec.charged,
                    stoppable,
                    expiry: voucher.expiry,
                    channel_id: id,
                    verdict: resp.action().into(),
                    reason: resp.reason,
                    toxic_score: resp.toxic_score,
                    status: status.as_u16(),
                    content_type,
                    data,
                    opened,
                }));
            }

            // A corrective 402: the seller's record of what it charged differs from ours. Adopt
            // its number only if we actually signed for it -- never a charge we did not authorise.
            if status == reqwest::StatusCode::PAYMENT_REQUIRED && data["error"] == errors::CUMULATIVE_MISMATCH && !retried {
                let theirs = data["accepts"]
                    .as_array()
                    .and_then(|a| a.iter().find_map(|o| o["extra"]["channelState"]["chargedCumulativeAmount"].as_str()))
                    .and_then(|s| parse_amount(s).ok());
                if let Some(theirs) = theirs.filter(|t| *t >= rec.claimed && *t <= rec.signed.max(charged)) {
                    rec.charged = theirs;
                    self.store.put_tab(rec).await?;
                    retried = true;
                    continue;
                }
            }
            if status == reqwest::StatusCode::FORBIDDEN && data["error"] == "payer_refused" {
                return Ok(Outcome::Refused(Refused {
                    url: url.into(),
                    seller: Some(offer.terms.pay_to),
                    verdict: "refused_by_seller".into(),
                    reason: format!("the seller screened this wallet and refused it: {}", data["reason"].as_str().unwrap_or("no reason given")),
                    toxic_score: data["toxicScore"].as_f64().unwrap_or(-1.0),
                }));
            }
            return Ok(Outcome::Unpayable {
                url: url.into(),
                reason: format!(
                    "the seller answered {status}: {}",
                    data.get("message").or_else(|| data.get("error")).or_else(|| data.get("reason")).map(|v| v.to_string()).unwrap_or(text)
                ),
            });
        }
    }

    async fn needs_approval(&self, account: &Account, url: &str, offer: &Offer, service: &Option<String>, r: CountersignResponse) -> Outcome {
        let Some(a) = r.approval.clone() else {
            return Outcome::Refused(refused(url, offer, &r));
        };
        self.pending.write().await.insert(a.approval_id.clone(), Pending { account: account.id.clone(), url: url.into() });
        let out = NeedsApproval {
            url: url.into(),
            approval_url: a.approval_url.clone().unwrap_or_else(|| a.verification_uri_complete.clone().unwrap_or(a.verification_uri.clone())),
            world_url: a.verification_uri_complete.clone().unwrap_or(a.verification_uri.clone()),
            approval_id: a.approval_id,
            user_code: a.user_code,
            expires_in: a.expires_in,
            seller: offer.terms.pay_to,
            service: service.clone(),
            limit: a.ceiling,
            requested: a.requested,
            approved_so_far: a.approved_so_far,
            reason: r.reason,
            toxic_score: r.toxic_score,
        };
        self.feed
            .tab("approval_needed", json!({"wallet": account.wallet, "seller": out.seller, "service": out.service,
                                            "approvalId": out.approval_id, "approvalUrl": out.approval_url, "limit": out.limit}))
            .await;
        Outcome::NeedsApproval(out)
    }

    async fn feed_refused(&self, account: &Account, url: &str, offer: &Offer, r: &CountersignResponse) {
        self.feed
            .tab("purchase_refused", json!({"wallet": account.wallet, "seller": offer.terms.pay_to, "url": url,
                                             "verdict": r.action(), "reason": r.reason, "toxicScore": r.toxic_score}))
            .await;
    }

    /// Wait (up to `timeout`) for the person to decide on an approval this account asked for,
    /// and finish the purchase if they said yes. `on_tick` is called while still waiting.
    pub async fn wait(&self, account: &Account, approval_id: &str, timeout: Duration, mut on_tick: impl FnMut(u64)) -> Outcome {
        let Some(p) = self.pending.read().await.get(approval_id).cloned() else {
            return Outcome::Denied { approval_id: approval_id.into(), reason: "no purchase is waiting on that approval".into() };
        };
        // an approval is only ever redeemed by the account that asked for it
        if p.account != account.id {
            return Outcome::Denied { approval_id: approval_id.into(), reason: "no purchase is waiting on that approval".into() };
        }
        let started = std::time::Instant::now();
        loop {
            match self.brain.approval(approval_id).await {
                Ok(Some(v)) => match v.status.as_str() {
                    "approved" => {
                        let outcome = self.buy(account, &p.url, Some(approval_id)).await;
                        self.pending.write().await.remove(approval_id);
                        return outcome;
                    }
                    "pending" => {}
                    other => {
                        self.pending.write().await.remove(approval_id);
                        let reason = v.denied_reason.unwrap_or_else(|| other.to_string());
                        return Outcome::Denied { approval_id: approval_id.into(), reason };
                    }
                },
                Ok(None) => return Outcome::Denied { approval_id: approval_id.into(), reason: "the approval no longer exists".into() },
                Err(e) => tracing::warn!("reading approval {approval_id}: {e:#}"),
            }
            if started.elapsed() >= timeout {
                let v = self.brain.approval(approval_id).await.ok().flatten();
                return Outcome::StillPending {
                    approval_id: approval_id.into(),
                    approval_url: v.as_ref().map(|v| v.world_link().to_string()).unwrap_or_default(),
                    user_code: v.map(|v| v.user_code).unwrap_or_default(),
                };
            }
            on_tick(started.elapsed().as_secs());
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn refused(url: &str, offer: &Offer, r: &CountersignResponse) -> Refused {
    Refused {
        url: url.into(),
        seller: Some(offer.terms.pay_to),
        verdict: r.action().to_string(),
        reason: r.reason.clone(),
        toxic_score: r.toxic_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sellers_name_comes_from_its_402() {
        let h = include_str!("../../agent/fixtures/402-hyperextend.b64").trim();
        assert_eq!(service_name(h).as_deref(), Some("hyperextend"));
        let o = include_str!("../../agent/fixtures/402-onesource.b64").trim();
        assert_eq!(service_name(o).as_deref(), Some("Latest Ethereum block height"));
        assert!(service_name("not base64").is_none());
    }

    #[test]
    fn outcomes_serialize_with_a_tag_for_the_ui_and_the_agent() {
        let o = Outcome::Unpayable { url: "https://x".into(), reason: "why".into() };
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(v["outcome"], "unpayable");
        assert_eq!(v["reason"], "why");
    }
}
