//! Screening the payer: the paid side of Intercepta's brief.
//!
//! A batch-settlement seller serves first and gets paid later, so every request extends
//! credit. The payer's risk decides how much: a clean payer may run up unclaimed value, an
//! elevated one is claimed after every request, a toxic one is not served at all.
//! Score -> credit line, not score -> boolean.

use crate::env::ScreenConfig;
use alloy::primitives::Address;
use common::intercepta::Intercepta;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Standing {
    /// Serve, and carry unclaimed value up to the configured limit.
    Trusted,
    /// Serve, but carry nothing: claim after every request.
    Careful,
    /// Do not serve.
    Refused,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub standing: Standing,
    /// -1 when screening was unavailable.
    pub toxic_score: f64,
    pub reason: String,
}

pub fn classify(score: f64, cfg: &ScreenConfig) -> Standing {
    if score >= cfg.refuse_at {
        Standing::Refused
    } else if score >= cfg.careful_at {
        Standing::Careful
    } else {
        Standing::Trusted
    }
}

pub struct PayerScreen {
    intercepta: Intercepta,
    cfg: ScreenConfig,
    cache: RwLock<HashMap<Address, (Instant, Verdict)>>,
}

impl PayerScreen {
    pub fn new(intercepta: Intercepta, cfg: ScreenConfig) -> Self {
        Self { intercepta, cfg, cache: RwLock::new(HashMap::new()) }
    }

    /// Screen one payer. Fails CLOSED: no verdict means no service.
    pub async fn verdict(&self, payer: Address) -> Verdict {
        let ttl = Duration::from_secs(self.cfg.cache_secs);
        if let Some((at, v)) = self.cache.read().await.get(&payer) {
            if at.elapsed() < ttl {
                return v.clone();
            }
        }

        let v = match self.intercepta.scan_address(&format!("{payer:#x}")).await {
            Ok(scan) => Verdict {
                standing: classify(scan.toxic_score, &self.cfg),
                toxic_score: scan.toxic_score,
                reason: scan.reason(),
            },
            // not cached: the next request tries again
            Err(e) => {
                return Verdict {
                    standing: Standing::Refused,
                    toxic_score: -1.0,
                    reason: format!("payer screening unavailable, failing closed: {e}"),
                }
            }
        };
        self.cache.write().await.insert(payer, (Instant::now(), v.clone()));
        v
    }
}

#[cfg(test)]
impl PayerScreen {
    /// Pre-load a verdict, for tests that exercise what happens after screening.
    pub async fn seed(&self, payer: Address, v: Verdict) {
        self.cache.write().await.insert(payer, (Instant::now(), v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ScreenConfig {
        ScreenConfig { refuse_at: 60.0, careful_at: 30.0, cache_secs: 60 }
    }

    #[test]
    fn score_sets_the_credit_line() {
        assert_eq!(classify(0.0, &cfg()), Standing::Trusted);
        assert_eq!(classify(29.9, &cfg()), Standing::Trusted);
        assert_eq!(classify(30.0, &cfg()), Standing::Careful);
        assert_eq!(classify(59.9, &cfg()), Standing::Careful);
        assert_eq!(classify(60.0, &cfg()), Standing::Refused);
    }

    #[tokio::test]
    async fn fails_closed_without_a_key() {
        let s = PayerScreen::new(Intercepta::new(String::new()), cfg());
        let v = s.verdict(Address::ZERO).await;
        assert_eq!(v.standing, Standing::Refused);
        assert!(v.reason.contains("failing closed"), "{}", v.reason);
        assert!(s.cache.read().await.is_empty(), "an outage must not be cached as a verdict");
    }
}
