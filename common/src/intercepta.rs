//! Live Intercepta (Web3 Antivirus) risk screening. Shared by both sides of a payment:
//! the countersigner screens the seller, the seller screens the payer.
//!
//! Endpoints verified live against `https://api.web3antivirus.io` (each returns HTTP 403
//! "This authentication key is incorrect or doesn't exist" without a key, which proves the
//! path is real):
//!
//!   GET  /api/public/v2/extension/account/{address}/quick-scan
//!   GET  /api/public/v2/extension/token-intelligence/token/{address}/risks?chainId={id}
//!   POST /api/public/v2/extension/analysis/signature
//!
//! Intercepta's risk data covers MAINNET, so we always screen mainnet addresses even when
//! a payment settles elsewhere.
//!
//! There is deliberately no mock mode. Without a key the client fails CLOSED: every screen
//! returns an error and the policy engine refuses to countersign. A guard that silently
//! allows traffic when its data source is down is not a guard.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::time::Duration;

const BASE: &str = "https://api.web3antivirus.io";

#[derive(Clone)]
pub struct Intercepta {
    http: reqwest::Client,
    api_key: String,
}

/// `GET /account/{address}/quick-scan`
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressScan {
    #[serde(default)]
    pub toxic_score: f64,
    #[serde(default)]
    pub traits: Vec<Trait>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trait {
    #[serde(default)]
    pub risk: f64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub txs_count: u64,
    #[serde(default)]
    pub description: String,
}

/// `GET /token-intelligence/token/{address}/risks`
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenScan {
    #[serde(default)]
    pub risk_score: f64,
    #[serde(default)]
    pub risk_level: String, // neutral | low | medium | high
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub trust: String, // whitelist | blocklist | neutral
    #[serde(default)]
    pub action: String, // block | warn | info
    #[serde(default)]
    pub detectors: Vec<Detector>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detector {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub description: String,
}

/// `POST /analysis/signature`
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageScan {
    #[serde(default)]
    pub risk_group: String, // Low | Medium | High
    #[serde(default)]
    pub message_type: String,
    #[serde(default)]
    pub detectors: Vec<Detector>,
}

impl Intercepta {
    pub fn new(api_key: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .build()
                .expect("http client"),
            api_key,
        }
    }

    fn ensure_key(&self) -> Result<()> {
        if self.api_key.trim().is_empty() {
            return Err(anyhow!(
                "no INTERCEPTA_API_KEY set - failing closed. Get a free key at https://intercepta.io/ethglobal"
            ));
        }
        Ok(())
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: String) -> Result<T> {
        self.ensure_key()?;
        let res = self
            .http
            .get(&url)
            .header("X-API-KEY", &self.api_key)
            .send()
            .await
            .with_context(|| format!("intercepta GET {url}"))?;
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("intercepta {status}: {body}"));
        }
        serde_json::from_str::<T>(&body)
            .with_context(|| format!("decoding intercepta response: {body}"))
    }

    /// Screen a counterparty wallet (the x402 `payTo`, or an inbound payer).
    pub async fn scan_address(&self, address: &str) -> Result<AddressScan> {
        self.get(format!(
            "{BASE}/api/public/v2/extension/account/{address}/quick-scan"
        ))
        .await
    }

    /// Screen the payment token - catches lookalike USDC.
    pub async fn scan_token(&self, address: &str, chain_id: u64) -> Result<TokenScan> {
        self.get(format!(
            "{BASE}/api/public/v2/extension/token-intelligence/token/{address}/risks?chainId={chain_id}"
        ))
        .await
    }

    /// Screen the payment authorization itself (EIP-712 / Permit2 payloads).
    pub async fn scan_message(
        &self,
        from: &str,
        message: &serde_json::Value,
        chain_id: u64,
    ) -> Result<MessageScan> {
        self.ensure_key()?;
        let url = format!("{BASE}/api/public/v2/extension/analysis/signature");
        let res = self
            .http
            .post(&url)
            .header("X-API-KEY", &self.api_key)
            .json(&serde_json::json!({
                "from": from,
                "message": message.to_string(),
                "chainId": chain_id.to_string(),
            }))
            .send()
            .await
            .context("intercepta POST analysis/signature")?;
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("intercepta {status}: {body}"));
        }
        serde_json::from_str::<MessageScan>(&body)
            .with_context(|| format!("decoding signature scan: {body}"))
    }
}

impl AddressScan {
    /// Highest-risk trait, used as the human-readable reason shown in the flow.
    pub fn worst_trait(&self) -> Option<&Trait> {
        self.traits
            .iter()
            .max_by(|a, b| a.risk.partial_cmp(&b.risk).unwrap_or(std::cmp::Ordering::Equal))
    }

    pub fn reason(&self) -> String {
        match self.worst_trait() {
            Some(t) if !t.description.is_empty() => {
                format!("{} ({})", t.description, t.name)
            }
            Some(t) => t.name.clone(),
            None => "no adverse signals".to_string(),
        }
    }
}
