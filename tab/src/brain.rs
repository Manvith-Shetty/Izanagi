//! Tab's line to the countersigner's operator API.
//!
//! Everything Tab may ask of it: read a wallet, screen a counterparty, close a tab, ask the
//! wallet's human to reopen one. Nothing here can make a payment happen -- the countersigner
//! decides that, per voucher, on the agent-facing endpoint.

use crate::feed::Feed;
use alloy::primitives::Address;
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// An approval as the countersigner shows it: no device code, no subject.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    pub id: String,
    pub purpose: String,
    pub status: String,
    pub wallet: String,
    pub seller: String,
    pub amount: u128,
    pub reason: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub created_at: u64,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orb_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx: Option<String>,
}

/// Where a handover approval stands.
#[derive(Debug, PartialEq)]
pub enum Handover {
    /// The wallet's human approved this exact new owner; the approval is now spent.
    Approved,
    Pending,
    /// Denied, expired, already used, or approved for a different account.
    Refused(String),
}

/// What a verified enrolment yields: a stable, private fingerprint of the person, and a
/// one-time grant to bind a wallet to them. Never the person's identity.
#[derive(Debug, Clone, Deserialize)]
pub struct Enrolled {
    pub human: String,
    pub grant: String,
}

impl ApprovalView {
    /// World's own page, with the code pre-filled when World offers that.
    pub fn world_link(&self) -> &str {
        self.verification_uri_complete.as_deref().unwrap_or(&self.verification_uri)
    }
}

#[derive(Clone)]
pub struct Brain {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Brain {
    pub fn new(base: &str, token: &str) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"),
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<(StatusCode, Value)> {
        let res = req.bearer_auth(&self.token).send().await.context("countersigner unreachable")?;
        let status = res.status();
        let body = res.json::<Value>().await.unwrap_or(Value::Null);
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND && body["error"].as_str().is_some_and(|e| e.contains("CONTROL_TOKEN")) {
            return Err(anyhow!("the countersigner rejected Tab's control token: {}", body["error"]));
        }
        Ok((status, body))
    }

    fn ok(status: StatusCode, body: Value) -> Result<Value> {
        if status.is_success() {
            Ok(body)
        } else {
            Err(anyhow!("{}", body["error"].as_str().or(body["reason"].as_str()).unwrap_or("countersigner error")))
        }
    }

    pub async fn health(&self) -> Result<Value> {
        let (s, b) = self.send(self.http.get(format!("{}/health", self.base))).await?;
        Self::ok(s, b)
    }

    pub async fn wallet(&self, wallet: Address) -> Result<Value> {
        let (s, b) = self.send(self.http.get(format!("{}/v1/wallets/{wallet:#x}", self.base))).await?;
        Self::ok(s, b)
    }

    pub async fn screen(&self, address: Address) -> Result<Value> {
        let (s, b) = self.send(self.http.get(format!("{}/v1/screen/{address:#x}", self.base))).await?;
        Self::ok(s, b)
    }

    pub async fn approval(&self, id: &str) -> Result<Option<ApprovalView>> {
        let (s, b) = self.send(self.http.get(format!("{}/v1/approvals/{id}", self.base))).await?;
        if s == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(Self::ok(s, b)?).context("decoding an approval")?))
    }

    pub async fn revoke(&self, wallet: Address, seller: Address, reason: Option<&str>) -> Result<Value> {
        let body = json!({"wallet": wallet, "seller": seller, "reason": reason});
        let (s, b) = self.send(self.http.post(format!("{}/v1/revoke", self.base)).json(&body)).await?;
        Self::ok(s, b)
    }

    /// Starts a World ID approval; nothing is restored until the wallet's human approves.
    pub async fn restore(&self, wallet: Address, seller: Address) -> Result<Value> {
        let body = json!({"wallet": wallet, "seller": seller});
        let (s, b) = self.send(self.http.post(format!("{}/v1/restore", self.base)).json(&body)).await?;
        Self::ok(s, b)
    }

    /// Start "prove you are a unique human" for someone who wants a Tab.
    pub async fn enroll(&self) -> Result<ApprovalView> {
        let (s, b) = self.send(self.http.post(format!("{}/v1/enroll", self.base))).await?;
        Ok(serde_json::from_value(Self::ok(s, b)?).context("decoding an enrolment")?)
    }

    /// Redeem an approved enrolment. `None` while the person has not finished verifying.
    pub async fn claim_enrollment(&self, id: &str) -> Result<Option<Enrolled>> {
        let (s, b) = self.send(self.http.post(format!("{}/v1/enroll/{id}/claim", self.base))).await?;
        if s == StatusCode::ACCEPTED {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(Self::ok(s, b)?).context("decoding a claimed enrolment")?))
    }

    /// Tie a freshly deployed wallet to the person an enrolment verified.
    pub async fn bind(&self, grant: &str, wallet: Address) -> Result<()> {
        let body = json!({"grant": grant, "wallet": wallet});
        let (s, b) = self.send(self.http.post(format!("{}/v1/bind", self.base)).json(&body)).await?;
        Self::ok(s, b).map(|_| ())
    }

    /// Ask the wallet's human, through World ID, to hand the wallet to `new_owner`.
    pub async fn handover(&self, wallet: Address, new_owner: Address) -> Result<ApprovalView> {
        let body = json!({"wallet": wallet, "newOwner": new_owner});
        let (s, b) = self.send(self.http.post(format!("{}/v1/handover", self.base)).json(&body)).await?;
        Ok(serde_json::from_value(Self::ok(s, b)?).context("decoding a handover approval")?)
    }

    /// Redeem a handover approval for exactly (wallet, new owner). Spends it when approved.
    pub async fn claim_handover(&self, id: &str, wallet: Address, new_owner: Address) -> Result<Handover> {
        let body = json!({"wallet": wallet, "newOwner": new_owner});
        let (s, b) = self.send(self.http.post(format!("{}/v1/handover/{id}/claim", self.base)).json(&body)).await?;
        Ok(match s {
            StatusCode::OK => Handover::Approved,
            StatusCode::ACCEPTED => Handover::Pending,
            StatusCode::FORBIDDEN => Handover::Refused(b["error"].as_str().unwrap_or("not approved").to_string()),
            _ => return Err(anyhow!("claiming a handover: countersigner answered {s}: {b}")),
        })
    }

    pub async fn activity(&self, after: u64) -> Result<Vec<Value>> {
        let (s, b) = self.send(self.http.get(format!("{}/v1/activity?after={after}", self.base))).await?;
        let b = Self::ok(s, b)?;
        Ok(b["entries"].as_array().cloned().unwrap_or_default())
    }

    /// Relay the countersigner's journal into `feed`, forever.
    ///
    /// Live over SSE; after any disconnect it first replays what it missed from
    /// `/v1/activity`, so a restart of either side never silently drops a revocation.
    pub async fn follow(self, feed: Arc<Feed>) {
        let mut last = 0u64;
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.activity(last).await {
                Ok(missed) => {
                    for e in missed {
                        last = last.max(e["id"].as_u64().unwrap_or(0));
                        feed.brain(e).await;
                    }
                }
                Err(e) => tracing::warn!("countersigner activity unavailable: {e:#}"),
            }
            match self.stream_into(&feed, &mut last).await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(e) => {
                    tracing::warn!("countersigner stream dropped, reconnecting in {backoff:?}: {e:#}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    async fn stream_into(&self, feed: &Feed, last: &mut u64) -> Result<()> {
        let res = reqwest::Client::new()
            .get(format!("{}/v1/stream", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("connecting to the countersigner stream")?;
        if !res.status().is_success() {
            return Err(anyhow!("countersigner stream answered {}", res.status()));
        }
        let mut bytes = res.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = bytes.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk.context("reading the stream")?));
            while let Some(end) = buf.find("\n\n") {
                let frame: String = buf.drain(..end + 2).collect();
                match parse_frame(&frame) {
                    Some(("entry", data)) => {
                        if let Ok(v) = serde_json::from_str::<Value>(data) {
                            let id = v["id"].as_u64().unwrap_or(0);
                            if id > *last {
                                *last = id;
                                feed.brain(v).await;
                            }
                        }
                    }
                    // fell behind the broadcast buffer: catch up from the durable list
                    Some(("lagged", _)) => {
                        for e in self.activity(*last).await? {
                            *last = (*last).max(e["id"].as_u64().unwrap_or(0));
                            feed.brain(e).await;
                        }
                    }
                    _ => {}
                }
            }
        }
        Err(anyhow!("stream closed"))
    }
}

/// `(event, data)` from one SSE frame. Comments and keep-alives have no event.
fn parse_frame(frame: &str) -> Option<(&str, &str)> {
    let mut event = None;
    let mut data = None;
    for line in frame.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            event = Some(v.trim());
        } else if let Some(v) = line.strip_prefix("data:") {
            data = Some(v.trim_start());
        }
    }
    Some((event?, data.unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_parse_into_event_and_data() {
        assert_eq!(parse_frame("id: 3\nevent: entry\ndata: {\"id\":3}\n\n"), Some(("entry", "{\"id\":3}")));
        assert_eq!(parse_frame("event: lagged\ndata: 12\n\n"), Some(("lagged", "12")));
        assert_eq!(parse_frame(": keep-alive\n\n"), None);
    }

    #[test]
    fn an_approval_view_decodes_from_the_countersigner_shape() {
        let v: ApprovalView = serde_json::from_value(json!({
            "id": "apr_1", "purpose": "payment", "status": "pending",
            "wallet": "0x1", "seller": "0x2", "amount": 100000, "reason": "over the limit",
            "userCode": "ABCD-EFGH", "verificationUri": "https://sandbox.auth.world.org/device",
            "createdAt": 1, "expiresAt": 2
        }))
        .unwrap();
        assert_eq!(v.world_link(), "https://sandbox.auth.world.org/device");
    }
}
