//! Discovery against the REAL Coinbase x402 Bazaar.
//!
//! No auth, no account: any agent can pull a list of strangers who will take its money.
//! Measured on this endpoint: 1,000+ live listings, 408 distinct seller addresses, 26
//! networks, a median price of $0.01 -- and 105 price entries asking $1,000 or more per
//! call, the largest of them $4,000,000,000,000.
//!
//! That is the problem Countersign exists for: the agent picks its counterparty at runtime,
//! from a directory neither it nor its owner has ever vetted.

use anyhow::{Context, Result};
use serde::Deserialize;

const DISCOVERY: &str = "https://api.cdp.coinbase.com/platform/v2/x402/discovery/resources";

#[derive(Debug, Clone, Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub resource: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub accepts: Vec<Accept>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Accept {
    #[serde(default)]
    pub amount: String,
    #[serde(default, rename = "payTo")]
    pub pay_to: Option<String>,
    #[serde(default)]
    pub asset: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
}

impl Accept {
    pub fn amount_atomic(&self) -> u128 {
        self.amount.parse().unwrap_or(0)
    }
}

#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default)]
    items: Vec<Listing>,
}

/// Fetch live listings. `network` filters e.g. `eip155:8453` (Base mainnet).
pub async fn discover(limit: usize, network: Option<&str>) -> Result<Vec<Listing>> {
    let body = reqwest::Client::new()
        .get(format!("{DISCOVERY}?limit={limit}"))
        .send()
        .await
        .context("x402 Bazaar discovery")?
        .text()
        .await?;
    let page: Page = serde_json::from_str(&body).context("decoding Bazaar page")?;
    Ok(page
        .items
        .into_iter()
        .filter(|l| {
            network.is_none()
                || l.accepts
                    .iter()
                    .any(|a| a.network.as_deref() == network)
        })
        .collect())
}

/// Summary statistics over the live marketplace -- the opening slide of the demo.
pub struct BazaarStats {
    pub listings: usize,
    pub sellers: usize,
    pub networks: usize,
    pub median_price: u128,
    pub over_1k: usize,
    pub max_price: u128,
    pub max_resource: String,
}

pub fn stats(listings: &[Listing]) -> BazaarStats {
    use std::collections::HashSet;
    let mut prices: Vec<u128> = Vec::new();
    let mut sellers = HashSet::new();
    let mut networks = HashSet::new();
    let mut max = (0u128, String::new());
    for l in listings {
        for a in &l.accepts {
            let p = a.amount_atomic();
            prices.push(p);
            if p > max.0 {
                max = (p, l.resource.clone());
            }
            if let Some(s) = &a.pay_to {
                sellers.insert(s.to_lowercase());
            }
            if let Some(n) = &a.network {
                networks.insert(n.clone());
            }
        }
    }
    prices.sort_unstable();
    BazaarStats {
        listings: listings.len(),
        sellers: sellers.len(),
        networks: networks.len(),
        median_price: prices.get(prices.len() / 2).copied().unwrap_or(0),
        over_1k: prices.iter().filter(|p| **p >= 1_000_000_000).count(),
        max_price: max.0,
        max_resource: max.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_over_a_known_page() {
        let l = vec![Listing {
            resource: "https://example/api".into(),
            kind: "http".into(),
            accepts: vec![
                Accept { amount: "10000".into(), pay_to: Some("0xAbC".into()), asset: None, network: Some("eip155:8453".into()) },
                Accept { amount: "4000000000000000000".into(), pay_to: Some("0xabc".into()), asset: None, network: Some("eip155:8453".into()) },
            ],
        }];
        let s = stats(&l);
        assert_eq!(s.sellers, 1, "payTo is case-insensitive");
        assert_eq!(s.over_1k, 1);
        assert_eq!(s.max_price, 4_000_000_000_000_000_000);
    }
}
