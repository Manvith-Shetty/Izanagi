//! Intercepta, with a short memory.
//!
//! Every purchase screens the seller and the token, and the watcher re-screens every open
//! seller on a timer. Against a metered API (the event key allows 1,000 calls) that adds up,
//! so answers are reused briefly -- with two rules that keep the guard a guard:
//!
//!   * a failure is never cached: an outage must fail closed on the next request too, not be
//!     remembered as a verdict;
//!   * the watcher always asks fresh (`address_fresh`): its whole job is to notice a score
//!     that has moved, and a cached answer cannot move.

use common::intercepta::{AddressScan, Intercepta, MessageScan, TokenScan};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub struct Screener {
    intercepta: Intercepta,
    address_ttl: Duration,
    token_ttl: Duration,
    addresses: RwLock<HashMap<String, (Instant, AddressScan)>>,
    tokens: RwLock<HashMap<(String, u64), (Instant, TokenScan)>>,
}

impl Screener {
    pub fn new(intercepta: Intercepta, address_ttl: Duration, token_ttl: Duration) -> Self {
        Self {
            intercepta,
            address_ttl,
            token_ttl,
            addresses: RwLock::new(HashMap::new()),
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// A counterparty's current risk, reusing an answer younger than the address TTL.
    pub async fn address(&self, address: &str) -> anyhow::Result<AddressScan> {
        let key = address.to_ascii_lowercase();
        if let Some((at, scan)) = self.addresses.read().await.get(&key) {
            if at.elapsed() < self.address_ttl {
                return Ok(scan.clone());
            }
        }
        self.address_fresh(address).await
    }

    /// Always asks Intercepta, and refreshes what `address` will return.
    pub async fn address_fresh(&self, address: &str) -> anyhow::Result<AddressScan> {
        let scan = self.intercepta.scan_address(address).await?;
        self.addresses.write().await.insert(address.to_ascii_lowercase(), (Instant::now(), scan.clone()));
        Ok(scan)
    }

    /// A token's trust. Token intelligence moves slowly, so this is remembered for longer.
    pub async fn token(&self, token: &str, chain_id: u64) -> anyhow::Result<TokenScan> {
        let key = (token.to_ascii_lowercase(), chain_id);
        if let Some((at, scan)) = self.tokens.read().await.get(&key) {
            if at.elapsed() < self.token_ttl {
                return Ok(scan.clone());
            }
        }
        let scan = self.intercepta.scan_token(token, chain_id).await?;
        self.tokens.write().await.insert(key, (Instant::now(), scan.clone()));
        Ok(scan)
    }

    /// The payment authorization itself. Never cached: every authorization is different.
    pub async fn message(&self, from: &str, message: &serde_json::Value, chain_id: u64) -> anyhow::Result<MessageScan> {
        self.intercepta.scan_message(from, message, chain_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_outage_is_not_remembered_as_a_verdict() {
        // no key: every call fails, and nothing may be cached from a failure
        let s = Screener::new(Intercepta::new(String::new()), Duration::from_secs(60), Duration::from_secs(60));
        assert!(s.address("0xabc").await.is_err());
        assert!(s.addresses.read().await.is_empty());
        assert!(s.token("0xdef", 8453).await.is_err());
        assert!(s.tokens.read().await.is_empty());
    }
}
