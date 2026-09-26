//! The decision. Intercepta's brief says a screening verdict should let the flow
//! "pay, refuse, cap the amount or ask a human" -- so those are literally the four outcomes.

use crate::intercepta::{AddressScan, MessageScan, TokenScan};
use crate::screener::Screener;
use serde::Serialize;
use std::sync::Arc;

/// What the risk verdict decided.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Verdict {
    /// Clean. Countersign the ceiling the agent asked for.
    Pay { ceiling: u128 },
    /// Elevated but not malicious. Countersign a REDUCED ceiling.
    Cap { ceiling: u128, requested: u128 },
    /// Over the autonomous limit, or risky enough to need a person.
    /// No signature until a fresh World ID authentication comes back.
    Ask { ceiling: u128, why: AskReason },
    /// Malicious, sanctioned, or screening unavailable. No signature exists.
    Refuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AskReason {
    OverAutonomousLimit,
    ElevatedRisk,
    UnknownCounterparty,
}

/// Everything the policy looked at, surfaced so the flow can show the risk, not just the answer.
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub verdict: Verdict,
    pub reason: String,
    pub toxic_score: f64,
    pub seller: String,
    /// Intercepta trait name -> a small integer stored on-chain when we revoke.
    pub reason_code: u32,
    pub address_scan: Option<AddressScan>,
    pub token_scan: Option<TokenScan>,
    pub message_scan: Option<MessageScan>,
}

#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// Below this, a clean counterparty is paid with no human involved.
    pub autonomous_limit: u128,
    /// toxicScore at or above this is refused outright.
    pub refuse_at: f64,
    /// toxicScore at or above this needs a human.
    pub ask_at: f64,
    /// toxicScore at or above this gets capped.
    pub cap_at: f64,
    /// Ceiling applied when capping.
    pub capped_ceiling: u128,
    /// Reject a price this far above the Bazaar median (guards the $4T listings).
    pub max_price: u128,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            autonomous_limit: 20_000_000, // 20 USDC
            refuse_at: 60.0,
            ask_at: 30.0,
            cap_at: 10.0,
            capped_ceiling: 1_000_000, // 1 USDC
            max_price: 100_000_000,    // 100 USDC/call is already absurd; the live Bazaar median is $0.01
        }
    }
}

/// Map an Intercepta trait name to a compact on-chain reason code.
pub use common::escrow::reason_code;

pub struct PolicyEngine {
    pub screener: Arc<Screener>,
    pub cfg: PolicyConfig,
}

impl PolicyEngine {
    pub fn new(screener: Arc<Screener>, cfg: PolicyConfig) -> Self {
        Self { screener, cfg }
    }

    /// Screen a proposed x402 payment and decide what happens next.
    ///
    /// `seller` and `token` are screened as MAINNET addresses (Intercepta's data is mainnet),
    /// regardless of which chain the payment settles on. `authorization` is the typed data
    /// `payer` will sign.
    pub async fn evaluate(
        &self,
        payer: &str,
        seller: &str,
        token: &str,
        chain_id: u64,
        requested_ceiling: u128,
        authorization: Option<&serde_json::Value>,
    ) -> Decision {
        // ---- 1. the counterparty ----
        let address_scan = match self.screener.address(seller).await {
            Ok(s) => s,
            Err(e) => {
                // fail closed: no verdict means no signature
                return Decision {
                    verdict: Verdict::Refuse,
                    reason: format!("screening unavailable, failing closed: {e}"),
                    toxic_score: -1.0,
                    seller: seller.to_string(),
                    reason_code: 0,
                    address_scan: None,
                    token_scan: None,
                    message_scan: None,
                };
            }
        };

        let score = address_scan.toxic_score;
        let worst = address_scan.worst_trait().cloned();
        let code = worst.as_ref().map(|t| reason_code(&t.name)).unwrap_or(0);
        let mut reason = address_scan.reason();

        // ---- 2. the token: real USDC or a lookalike? ----
        let token_scan = self.screener.token(token, chain_id).await.ok();
        if let Some(ts) = &token_scan {
            if ts.action == "block" || ts.trust == "blocklist" {
                return Decision {
                    verdict: Verdict::Refuse,
                    reason: format!("token blocked by Intercepta: {} ({})", ts.category, ts.risk_level),
                    toxic_score: score,
                    seller: seller.to_string(),
                    reason_code: reason_code("honeypot"),
                    address_scan: Some(address_scan),
                    token_scan,
                    message_scan: None,
                };
            }
        }

        // ---- 3. the authorization payload itself ----
        let message_scan = match authorization {
            Some(msg) => self.screener.message(payer, msg, chain_id).await.ok(),
            None => None,
        };
        if let Some(ms) = &message_scan {
            if ms.risk_group.eq_ignore_ascii_case("high") {
                return Decision {
                    verdict: Verdict::Refuse,
                    reason: format!("payment authorization flagged high risk ({})", ms.message_type),
                    toxic_score: score,
                    seller: seller.to_string(),
                    reason_code: 99,
                    address_scan: Some(address_scan),
                    token_scan,
                    message_scan,
                };
            }
        }

        // ---- 4. price sanity, against the live Bazaar distribution ----
        if requested_ceiling > self.cfg.max_price {
            return Decision {
                verdict: Verdict::Refuse,
                reason: format!(
                    "price {} far above sane range (live x402 Bazaar median is 0.01 USDC)",
                    fmt_usdc(requested_ceiling)
                ),
                toxic_score: score,
                seller: seller.to_string(),
                reason_code: 99,
                address_scan: Some(address_scan),
                token_scan,
                message_scan,
            };
        }

        // ---- 5. the four outcomes ----
        let verdict = if score >= self.cfg.refuse_at {
            Verdict::Refuse
        } else if score >= self.cfg.ask_at {
            Verdict::Ask { ceiling: requested_ceiling, why: AskReason::ElevatedRisk }
        } else if score >= self.cfg.cap_at {
            let capped = requested_ceiling.min(self.cfg.capped_ceiling);
            Verdict::Cap { ceiling: capped, requested: requested_ceiling }
        } else if requested_ceiling > self.cfg.autonomous_limit {
            Verdict::Ask { ceiling: requested_ceiling, why: AskReason::OverAutonomousLimit }
        } else if address_scan.traits.is_empty() && requested_ceiling > self.cfg.capped_ceiling {
            // no signals at all, good or bad: 60 of 60 real x402 payers on Base have a
            // median of zero lifetime transactions, so "no history" is the normal case here.
            Verdict::Ask { ceiling: requested_ceiling, why: AskReason::UnknownCounterparty }
        } else {
            Verdict::Pay { ceiling: requested_ceiling }
        };

        if matches!(verdict, Verdict::Ask { why: AskReason::OverAutonomousLimit, .. }) {
            reason = format!(
                "{} exceeds the {} autonomous limit - needs a human",
                fmt_usdc(requested_ceiling),
                fmt_usdc(self.cfg.autonomous_limit)
            );
        }

        Decision {
            verdict,
            reason,
            toxic_score: score,
            seller: seller.to_string(),
            reason_code: code,
            address_scan: Some(address_scan),
            token_scan,
            message_scan,
        }
    }
}

pub fn fmt_usdc(v: u128) -> String {
    format!("{}.{:06} USDC", v / 1_000_000, v % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> PolicyEngine {
        let screener = Screener::new(
            crate::intercepta::Intercepta::new(String::new()),
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(60),
        );
        PolicyEngine::new(Arc::new(screener), PolicyConfig::default())
    }

    #[tokio::test]
    async fn fails_closed_without_an_api_key() {
        let d = engine()
            .evaluate("0x3", "0x1", "0x2", 8453, 1_000_000, None)
            .await;
        assert_eq!(d.verdict, Verdict::Refuse, "no screening data must mean no signature");
        assert!(d.reason.contains("failing closed"));
    }

    #[test]
    fn reason_codes_are_stable() {
        assert_eq!(reason_code("sanction_address"), 1);
        assert_eq!(reason_code("known_scammer"), 2);
        assert_eq!(reason_code("something_new"), 99);
    }

    #[test]
    fn usdc_formatting() {
        assert_eq!(fmt_usdc(20_000_000), "20.000000 USDC");
        assert_eq!(fmt_usdc(1), "0.000001 USDC");
    }
}
