//! The state of x402 batch-settlement on the live chain, measured rather than asserted.
//!
//! Every channel ever opened on Coinbase's escrow (from its `ChannelCreated` events, via
//! Blockscout), the USDC it holds right now, and how many of those channels have a payer that
//! can stop a payment after signing -- a Countersign wallet. Recomputed hourly and cached on
//! disk, so the landing page has numbers the moment Tab starts.

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{Context, Result};
use common::escrow::{ICountersign, IERC20};
use common::{ESCROW, USDC_BASE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::RwLock;

const CHANNEL_CREATED: &str = "0x69d8248d5566bdb2ebc4d218970710d8378a2ff9b709a999e052b71970f808fa";
/// The block the escrow was deployed at on Base.
const ESCROW_DEPLOYED: u64 = 46_033_952;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Census {
    pub measured_at: u64,
    pub channels: u64,
    pub payers: u64,
    pub sellers: u64,
    pub channels_last_14d: u64,
    /// USDC the escrow holds right now, atomic.
    pub escrow_usdc: u128,
    /// Channels whose payer is checked by EIP-1271 at claim time (`payerAuthorizer == 0`).
    pub policy_channels: u64,
    /// Of those, payers that are Countersign wallets: they can stop a payment after it is signed.
    pub countersign_payers: u64,
}

pub struct Censor {
    http: reqwest::Client,
    blockscout: String,
    rpc: String,
    path: PathBuf,
    current: RwLock<Census>,
}

impl Censor {
    pub fn new(blockscout: &str, rpc: &str, path: PathBuf) -> Self {
        let current = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(60)).build().expect("http client"),
            blockscout: blockscout.trim_end_matches('/').into(),
            rpc: rpc.into(),
            path,
            current: RwLock::new(current),
        }
    }

    pub async fn get(&self) -> Census {
        self.current.read().await.clone()
    }

    /// Recompute forever, hourly.
    pub async fn run(&self) {
        loop {
            match self.measure().await {
                Ok(c) => {
                    tracing::info!(channels = c.channels, payers = c.payers, "network census refreshed");
                    if let Ok(s) = serde_json::to_vec_pretty(&c) {
                        let _ = std::fs::write(&self.path, s);
                    }
                    *self.current.write().await = c;
                }
                Err(e) => tracing::warn!("network census failed, keeping the last one: {e:#}"),
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }

    async fn measure(&self) -> Result<Census> {
        let logs = self.channel_logs().await?;
        let now = common_now();
        let (mut payers, mut sellers, mut policy) = (HashSet::new(), HashSet::new(), HashSet::new());
        let (mut recent, mut policy_channels) = (0u64, 0u64);
        for l in &logs {
            let d = l["data"].as_str().unwrap_or("").trim_start_matches("0x");
            if d.len() < 64 * 3 {
                continue;
            }
            let word = |i: usize| format!("0x{}", &d[i * 64 + 24..(i + 1) * 64]);
            let (payer, payer_auth, receiver) = (word(0), word(1), word(2));
            payers.insert(payer.clone());
            sellers.insert(receiver);
            if payer_auth == format!("0x{}", "0".repeat(40)) {
                policy_channels += 1;
                policy.insert(payer);
            }
            let ts = l["timeStamp"].as_str().and_then(|t| u64::from_str_radix(t.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
            if now.saturating_sub(ts) <= 14 * 86400 {
                recent += 1;
            }
        }

        let p = ProviderBuilder::new().connect_http(self.rpc.parse().context("census RPC")?);
        let escrow_usdc = IERC20::new(USDC_BASE, &p).balanceOf(ESCROW).call().await.context("escrow balance")?.to::<u128>();
        // a policy payer is a Countersign wallet if it has code and answers riskOracle()
        let mut countersign = 0u64;
        for payer in policy {
            let Ok(a) = payer.parse::<Address>() else { continue };
            if p.get_code_at(a).await.map(|c| c.is_empty()).unwrap_or(true) {
                continue;
            }
            if ICountersign::new(a, &p).riskOracle().call().await.is_ok() {
                countersign += 1;
            }
        }
        Ok(Census {
            measured_at: now,
            channels: logs.len() as u64,
            payers: payers.len() as u64,
            sellers: sellers.len() as u64,
            channels_last_14d: recent,
            escrow_usdc,
            policy_channels,
            countersign_payers: countersign,
        })
    }

    /// Every ChannelCreated log, paging through Blockscout's 1,000-per-call limit by block.
    async fn channel_logs(&self) -> Result<Vec<Value>> {
        let mut out: Vec<Value> = Vec::new();
        let mut seen = HashSet::new();
        let mut from = ESCROW_DEPLOYED;
        for _ in 0..50 {
            let url = format!(
                "{}?module=logs&action=getLogs&address={ESCROW:#x}&topic0={CHANNEL_CREATED}&fromBlock={from}&toBlock=latest",
                self.blockscout
            );
            let page: Value = self.http.get(&url).send().await?.json().await?;
            let items = page["result"].as_array().cloned().unwrap_or_default();
            let n = items.len();
            for l in items {
                let key = format!("{}:{}", l["transactionHash"], l["logIndex"]);
                if seen.insert(key) {
                    from = from.max(u64::from_str_radix(l["blockNumber"].as_str().unwrap_or("0x0").trim_start_matches("0x"), 16).unwrap_or(from));
                    out.push(l);
                }
            }
            if n < 1000 {
                break;
            }
        }
        Ok(out)
    }
}

fn common_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}
