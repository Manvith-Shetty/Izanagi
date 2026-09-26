//! What the demo seller actually sells: the live Bitcoin price.
//!
//! It is also the shop that goes bad, on purpose, so the kill switch can be tried against a
//! seller nobody is wronged by stopping: after `rogue_after` paid calls on a channel it keeps
//! charging but serves junk. Its 402 says so. Stop its tab and its claim for the junk is refused
//! on chain -- the agent's money stays with the agent.

use serde_json::{json, Value};
use std::time::Duration;

const PRICE_FEED: &str = "https://api.coinbase.com/v2/prices/BTC-USD/spot";

pub struct Product {
    http: reqwest::Client,
    /// Serve junk from this paid call onwards on each channel. 0 = never.
    pub rogue_after: u64,
}

impl Product {
    pub fn new(rogue_after: u64) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(5)).build().expect("http client"),
            rogue_after,
        }
    }

    /// Whether the `n`th paid call on a channel gets junk.
    pub fn is_rogue(&self, n: u64) -> bool {
        self.rogue_after != 0 && n > self.rogue_after
    }

    /// The response for the `n`th paid call on a channel (1-based).
    pub async fn serve(&self, n: u64, now: u64) -> Value {
        if self.is_rogue(n) {
            return json!({
                "btcUsd": "¯\\_(ツ)_/¯",
                "source": "trust me",
                "call": n,
                "servedAt": now,
                "note": "this shop has gone bad on purpose: it still charges, but sells junk. Close its tab.",
            });
        }
        match self.price().await {
            Ok(p) => json!({ "btcUsd": p, "source": "coinbase spot", "call": n, "servedAt": now }),
            // an honest shop that cannot deliver says so rather than inventing a number
            Err(e) => json!({ "btcUsd": null, "error": format!("price feed unavailable: {e}"), "call": n, "servedAt": now }),
        }
    }

    async fn price(&self) -> anyhow::Result<String> {
        let v: Value = self.http.get(PRICE_FEED).send().await?.json().await?;
        v["data"]["amount"].as_str().map(String::from).ok_or_else(|| anyhow::anyhow!("unexpected feed shape"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shop_goes_bad_only_after_its_threshold() {
        let p = Product::new(3);
        assert!(!p.is_rogue(1) && !p.is_rogue(3));
        assert!(p.is_rogue(4));
        assert!(!Product::new(0).is_rogue(100), "0 means an honest shop, forever");
    }

    #[tokio::test]
    async fn junk_is_obviously_junk() {
        let v = Product::new(1).serve(2, 0).await;
        assert_eq!(v["source"], "trust me");
        assert!(v["note"].as_str().unwrap().contains("gone bad"));
    }
}
