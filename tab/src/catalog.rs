//! What an agent can buy through Tab.
//!
//! A short curated list first -- each entry says whether it has been paid through a
//! Countersign wallet on mainnet -- then every `batch-settlement` seller on our chain from
//! Coinbase's live x402 Bazaar. Tab can only pay batch-settlement sellers: it is the one x402
//! scheme where a payment can still be stopped after it is signed.

use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const BAZAAR: &str = "https://api.cdp.coinbase.com/platform/v2/x402/discovery/resources?limit=1000";
const REFRESH: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub url: String,
    pub name: String,
    pub description: String,
    /// Per call, atomic USDC.
    pub price: u128,
    pub seller: String,
    /// `curated` or `bazaar`.
    pub source: &'static str,
    /// Paid through a Countersign wallet on mainnet, with the transaction to prove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proven: Option<&'static str>,
    /// A shop we run to demonstrate the kill switch. Never revoke a real seller who served you.
    pub demo: bool,
}

fn curated(demo_shop: Option<&str>) -> Vec<Listing> {
    let mut v = vec![
        Listing {
            url: "https://api.hyperextend.xyz/v1/candles/BTC/1m/latest".into(),
            name: "hyperextend · BTC candles".into(),
            description: "The latest 1-minute Bitcoin candles from Hyperliquid (open, high, low, close, volume).".into(),
            price: 2000,
            seller: "0x548fC289526ab2F0391D562a723cdA64Bbf1abc4".into(),
            source: "curated",
            proven: Some("https://basescan.org/tx/0xafe253d7ac946616eae09f8e025371174412a5c5a62bb4303c9021a1365f1a99"),
            demo: false,
        },
        Listing {
            url: "https://api.onesource.io/api/chain/block-number".into(),
            name: "onesource · block number".into(),
            description: "The current Ethereum block height.".into(),
            price: 1000,
            seller: "0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea".into(),
            source: "curated",
            proven: Some("https://basescan.org/tx/0xd4d8fbb14bf4572dc5b35285646d64c6d3cf69a76d5b8dfbc3f2925c493eaec2"),
            demo: false,
        },
    ];
    if let Some(shop) = demo_shop {
        v.push(Listing {
            url: format!("{}/v1/data", shop.trim_end_matches('/')),
            name: "Tab demo shop · BTC price (goes bad)".into(),
            description: "Our own shop, for trying the kill switch: serves the live Bitcoin price, then starts \
                          selling junk after a few calls. Close its tab and its claim for the junk is refused on chain."
                .into(),
            price: 1000,
            seller: String::new(),
            source: "curated",
            proven: None,
            demo: true,
        });
    }
    v
}

pub struct Catalog {
    http: reqwest::Client,
    network: String,
    demo_shop: Option<String>,
    bazaar: RwLock<Option<(Instant, Vec<Listing>)>>,
}

impl Catalog {
    pub fn new(chain_id: u64, demo_shop: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"),
            network: format!("eip155:{chain_id}"),
            demo_shop,
            bazaar: RwLock::new(None),
        }
    }

    /// Curated first, then the live Bazaar, deduplicated by URL. `query` filters by words.
    pub async fn list(&self, query: Option<&str>) -> Vec<Listing> {
        let mut all = curated(self.demo_shop.as_deref());
        for l in self.bazaar().await {
            if !all.iter().any(|c| c.url == l.url) {
                all.push(l);
            }
        }
        match query.map(|q| q.to_lowercase()).filter(|q| !q.trim().is_empty()) {
            None => all,
            Some(q) => all
                .into_iter()
                .filter(|l| {
                    let hay = format!("{} {} {}", l.name, l.description, l.url).to_lowercase();
                    q.split_whitespace().all(|w| hay.contains(w))
                })
                .collect(),
        }
    }

    async fn bazaar(&self) -> Vec<Listing> {
        if let Some((at, v)) = self.bazaar.read().await.as_ref() {
            if at.elapsed() < REFRESH {
                return v.clone();
            }
        }
        match self.fetch().await {
            Ok(v) => {
                *self.bazaar.write().await = Some((Instant::now(), v.clone()));
                v
            }
            Err(e) => {
                tracing::warn!("Bazaar unavailable: {e:#}");
                // a stale list beats none
                self.bazaar.read().await.as_ref().map(|(_, v)| v.clone()).unwrap_or_default()
            }
        }
    }

    async fn fetch(&self) -> anyhow::Result<Vec<Listing>> {
        let page: Value = self.http.get(BAZAAR).send().await?.json().await?;
        Ok(parse_bazaar(&page, &self.network))
    }
}

/// Every batch-settlement offer on `network` in one Bazaar page.
pub fn parse_bazaar(page: &Value, network: &str) -> Vec<Listing> {
    let mut out = Vec::new();
    for item in page["items"].as_array().into_iter().flatten() {
        let url = item["resource"].as_str().unwrap_or_default();
        for a in item["accepts"].as_array().into_iter().flatten() {
            if a["scheme"] != "batch-settlement" || a["network"] != network {
                continue;
            }
            let host = url.split('/').nth(2).unwrap_or(url);
            let price = a["amount"].as_str().or(a["maxAmountRequired"].as_str()).and_then(|s| s.parse().ok()).unwrap_or(0);
            out.push(Listing {
                url: url.into(),
                name: host.into(),
                description: a["description"].as_str().or(item["metadata"]["description"].as_str()).unwrap_or("").chars().take(200).collect(),
                price,
                seller: a["payTo"].as_str().unwrap_or_default().into(),
                source: "bazaar",
                proven: None,
                demo: false,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_batch_settlement_on_our_network_is_listed() {
        let page = json!({"items": [
            {"resource": "https://a.example/x", "accepts": [
                {"scheme": "exact", "network": "eip155:8453", "amount": "10", "payTo": "0x1"},
                {"scheme": "batch-settlement", "network": "eip155:8453", "amount": "2000", "payTo": "0x2"}]},
            {"resource": "https://b.example/y", "accepts": [
                {"scheme": "batch-settlement", "network": "eip155:10", "amount": "5", "payTo": "0x3"}]}
        ]});
        let l = parse_bazaar(&page, "eip155:8453");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].price, 2000);
        assert_eq!(l[0].name, "a.example");
    }

    #[tokio::test]
    async fn curated_entries_come_first_and_filter_by_words() {
        let c = Catalog::new(8453, Some("https://shop.example".into()));
        // pre-fill the cache so the test does not touch the network
        *c.bazaar.write().await = Some((Instant::now(), vec![]));
        let all = c.list(None).await;
        assert_eq!(all[0].source, "curated");
        assert!(all.iter().any(|l| l.demo && l.url == "https://shop.example/v1/data"));
        let btc = c.list(Some("btc candles")).await;
        assert_eq!(btc.len(), 1);
        assert!(btc[0].proven.is_some());
    }
}
