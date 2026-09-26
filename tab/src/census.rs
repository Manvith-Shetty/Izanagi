//! The state of x402 batch-settlement on the live chain, measured rather than asserted.
//!
//! Every channel ever opened on Coinbase's escrow (its `ChannelCreated` events), the USDC it
//! holds right now, and how many of those channels have a payer that can stop a payment after
//! signing -- a Countersign wallet. Recomputed hourly and cached on disk, so the landing page
//! has numbers the moment Tab starts.
//!
//! History comes from Blockscout, which pages cheaply but was measured missing events emitted
//! inside a contract call (a Countersign wallet opening a channel is exactly that). So the
//! last two days are always re-read straight from the chain and merged in.

use alloy::primitives::{Address, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::client::ClientBuilder;
use alloy::rpc::types::Filter;
use alloy::transports::layers::RetryBackoffLayer;
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
/// How far back the chain itself is re-read (~2 days of 2-second blocks).
const RECENT_BLOCKS: u64 = 86_400;
/// The most blocks a public RPC will scan in one eth_getLogs.
const LOG_WINDOW: u64 = 2_000;

/// One ChannelCreated event, from either source.
struct Created {
    key: String,
    data: String,
    /// Unix time, when the source gave one.
    at: Option<u64>,
}

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
                // channels are only ever added: a smaller count means a source dropped data
                Ok(c) if c.channels < self.current.read().await.channels => {
                    tracing::warn!(new = c.channels, "census came back smaller than the last one; keeping the last one");
                }
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
        let client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new(8, 800, 100))
            .http(self.rpc.parse().context("census RPC")?);
        let p = ProviderBuilder::new().connect_client(client);

        let mut events = self.channel_logs().await?;
        let mut seen: HashSet<String> = events.iter().map(|e| e.key.clone()).collect();
        for e in self.recent_from_chain(&p).await? {
            if seen.insert(e.key.clone()) {
                events.push(e);
            }
        }

        let now = common_now();
        let (mut payers, mut sellers, mut policy) = (HashSet::new(), HashSet::new(), HashSet::new());
        let (mut recent, mut policy_channels) = (0u64, 0u64);
        for e in &events {
            let d = e.data.trim_start_matches("0x");
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
            // events read straight from the chain are, by construction, from the last two days
            if e.at.map(|t| now.saturating_sub(t) <= 14 * 86400).unwrap_or(true) {
                recent += 1;
            }
        }

        let escrow_usdc = IERC20::new(USDC_BASE, &p).balanceOf(ESCROW).call().await.context("escrow balance")?.to::<u128>();
        // a policy payer is a Countersign wallet if it has code and answers riskOracle()
        let mut countersign = 0u64;
        for payer in policy {
            let Ok(a) = payer.parse::<Address>() else { continue };
            match p.get_code_at(a).await {
                Ok(c) if !c.is_empty() => {}
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!(payer = %a, "census could not read code: {e:#}");
                    continue;
                }
            }
            if ICountersign::new(a, &p).riskOracle().call().await.is_ok() {
                countersign += 1;
            }
        }
        Ok(Census {
            measured_at: now,
            channels: events.len() as u64,
            payers: payers.len() as u64,
            sellers: sellers.len() as u64,
            channels_last_14d: recent,
            escrow_usdc,
            policy_channels,
            countersign_payers: countersign,
        })
    }

    /// The last `RECENT_BLOCKS` of ChannelCreated events, read from the chain in RPC-sized windows.
    async fn recent_from_chain(&self, p: &impl Provider) -> Result<Vec<Created>> {
        let tip = p.get_block_number().await.context("latest block")?;
        let topic: B256 = CHANNEL_CREATED.parse()?;
        let mut out = Vec::new();
        let mut from = tip.saturating_sub(RECENT_BLOCKS);
        while from <= tip {
            let to = (from + LOG_WINDOW - 1).min(tip);
            let f = Filter::new().address(ESCROW).event_signature(topic).from_block(from).to_block(to);
            for l in p.get_logs(&f).await.with_context(|| format!("logs {from}..{to}"))? {
                let key = format!(
                    "\"{:#x}\":\"{:#x}\"",
                    l.transaction_hash.unwrap_or_default(),
                    l.log_index.unwrap_or_default()
                );
                out.push(Created { key, data: format!("{}", l.data().data), at: l.block_timestamp });
            }
            from = to + 1;
        }
        Ok(out)
    }

    /// Every ChannelCreated log Blockscout has, paging through its 1,000-per-call limit.
    async fn channel_logs(&self) -> Result<Vec<Created>> {
        let mut out: Vec<Created> = Vec::new();
        let mut seen = HashSet::new();
        let mut from = ESCROW_DEPLOYED;
        for _ in 0..50 {
            let url = format!(
                "{}?module=logs&action=getLogs&address={ESCROW:#x}&topic0={CHANNEL_CREATED}&fromBlock={from}&toBlock=latest",
                self.blockscout
            );
            // a page that fails is retried, and then fails the census: never counted as empty
            let mut items = None;
            for attempt in 0..4u64 {
                match self.http.get(&url).send().await {
                    Ok(r) => match r.json::<Value>().await {
                        Ok(page) if page["result"].is_array() => {
                            items = page["result"].as_array().cloned();
                            break;
                        }
                        Ok(page) => tracing::warn!("Blockscout page failed: {}", page["message"]),
                        Err(e) => tracing::warn!("Blockscout page unreadable: {e:#}"),
                    },
                    Err(e) => tracing::warn!("Blockscout unreachable: {e:#}"),
                }
                tokio::time::sleep(Duration::from_secs(3 * (attempt + 1))).await;
            }
            let items = items.ok_or_else(|| anyhow::anyhow!("Blockscout kept failing from block {from}"))?;
            let n = items.len();
            for l in items {
                let hex = |v: &Value| u64::from_str_radix(v.as_str().unwrap_or("0x0").trim_start_matches("0x"), 16).ok();
                // the same key shape as the chain path, so the two sources deduplicate
                let key = format!(
                    "\"{}\":\"{:#x}\"",
                    l["transactionHash"].as_str().unwrap_or("").to_lowercase(),
                    hex(&l["logIndex"]).unwrap_or(0)
                );
                if seen.insert(key.clone()) {
                    from = from.max(hex(&l["blockNumber"]).unwrap_or(from));
                    out.push(Created { key, data: l["data"].as_str().unwrap_or("").to_string(), at: hex(&l["timeStamp"]) });
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
