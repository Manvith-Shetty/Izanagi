//! Open sessions: every channel we are currently countersigning for.
//!
//! This is the part that makes Countersign different. Every other agent-payment guard
//! screens once, before the agent signs, and is then committed. We keep screening for as
//! long as a session is open, because the seller only cashes in at the end -- so a verdict
//! that arrives late still counts. The loop that does it is `App::watch`.

use alloy::primitives::Address;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// A channel the agent is currently paying through.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub channel_id: String,
    /// The Countersign wallet paying (the channel's `payer`).
    pub wallet: String,
    pub seller: String,
    pub token: String,
    pub chain_id: u64,
    /// Highest ceiling we have countersigned so far.
    pub ceiling: u128,
    /// How many vouchers we have countersigned in this session.
    pub vouchers: u64,
    /// When the newest attestation lapses. After it, nothing we signed here is claimable.
    pub expiry: u64,
    pub opened_at: u64,
    pub last_screened_at: u64,
    pub last_score: f64,
    pub revoked: bool,
    pub revoked_reason: Option<String>,
}

/// What stopping a seller cost them, across one wallet's sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stopped {
    pub vouchers: u64,
    /// Highest live countersigned ceiling across the stopped sessions.
    pub value: u128,
    pub score_before: Option<f64>,
}

type Key = (Address, Address); // (wallet, seller)

#[derive(Default)]
pub struct Sessions {
    inner: RwLock<HashMap<String, Session>>,
    /// Sellers a wallet will not countersign for, paid or not.
    blocked: RwLock<HashSet<Key>>,
    /// Per channel: the cumulative limit a person has approved it to run up to.
    limits: RwLock<HashMap<String, u128>>,
}

/// The limit a person is asked to approve for a tab that wants `ceiling`: the next multiple
/// of `step` at or above it, so one approval covers the calls that follow.
pub fn approval_limit(ceiling: u128, step: u128) -> u128 {
    if step == 0 {
        return ceiling;
    }
    ceiling.div_ceil(step).saturating_mul(step)
}

impl Sessions {
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        &self,
        channel_id: &str,
        wallet: Address,
        seller: Address,
        token: Address,
        chain_id: u64,
        ceiling: u128,
        expiry: u64,
        score: f64,
    ) {
        let mut g = self.inner.write().await;
        let e = g.entry(channel_id.to_string()).or_insert_with(|| Session {
            channel_id: channel_id.to_string(),
            wallet: format!("{wallet:#x}"),
            seller: format!("{seller:#x}"),
            token: format!("{token:#x}"),
            chain_id,
            ceiling: 0,
            vouchers: 0,
            expiry,
            opened_at: now(),
            last_screened_at: now(),
            last_score: score,
            revoked: false,
            revoked_reason: None,
        });
        e.ceiling = e.ceiling.max(ceiling);
        e.expiry = e.expiry.max(expiry);
        e.vouchers += 1;
        e.last_score = score;
        e.last_screened_at = now();
    }

    /// The limit a person approved for this channel; zero if none has.
    pub async fn approved_limit(&self, channel_id: &str) -> u128 {
        self.limits.read().await.get(channel_id).copied().unwrap_or(0)
    }

    /// Record a person's approval. Returns the previous limit. Never lowers it.
    pub async fn raise_limit(&self, channel_id: &str, limit: u128) -> u128 {
        let mut g = self.limits.write().await;
        let prev = g.get(channel_id).copied().unwrap_or(0);
        g.insert(channel_id.to_string(), prev.max(limit));
        prev
    }

    pub async fn list(&self) -> Vec<Session> {
        self.inner.read().await.values().cloned().collect()
    }

    pub async fn for_wallet(&self, wallet: Address) -> Vec<Session> {
        let w = format!("{wallet:#x}");
        self.inner.read().await.values().filter(|s| s.wallet == w).cloned().collect()
    }

    pub async fn is_revoked(&self, wallet: Address, seller: Address) -> bool {
        self.blocked.read().await.contains(&(wallet, seller))
    }

    pub async fn blocked_for(&self, wallet: Address) -> Vec<String> {
        self.blocked
            .read()
            .await
            .iter()
            .filter(|(w, _)| *w == wallet)
            .map(|(_, s)| format!("{s:#x}"))
            .collect()
    }

    /// Unique (wallet, seller) pairs still being paid, for the watcher.
    pub async fn open_pairs(&self) -> Vec<(Address, Address)> {
        let mut seen = HashSet::new();
        for s in self.inner.read().await.values().filter(|s| !s.revoked) {
            if let (Ok(w), Ok(sel)) = (s.wallet.parse(), s.seller.parse()) {
                seen.insert((w, sel));
            }
        }
        seen.into_iter().collect()
    }

    pub async fn record_score(&self, seller: Address, score: f64) {
        let s = format!("{seller:#x}");
        for sess in self.inner.write().await.values_mut().filter(|x| x.seller == s) {
            sess.last_score = score;
            sess.last_screened_at = now();
        }
    }

    /// Stop countersigning for `seller` on `wallet`, and report what was outstanding.
    pub async fn revoke(&self, wallet: Address, seller: Address, reason: &str) -> Stopped {
        self.blocked.write().await.insert((wallet, seller));
        let (w, s) = (format!("{wallet:#x}"), format!("{seller:#x}"));
        let mut out = Stopped::default();
        for sess in self.inner.write().await.values_mut() {
            if sess.wallet == w && sess.seller == s && !sess.revoked {
                sess.revoked = true;
                sess.revoked_reason = Some(reason.to_string());
                out.vouchers += sess.vouchers;
                out.value = out.value.max(sess.ceiling);
                out.score_before = Some(sess.last_score);
            }
        }
        out
    }

    pub async fn restore(&self, wallet: Address, seller: Address) {
        self.blocked.write().await.remove(&(wallet, seller));
        let (w, s) = (format!("{wallet:#x}"), format!("{seller:#x}"));
        for sess in self.inner.write().await.values_mut() {
            if sess.wallet == w && sess.seller == s {
                sess.revoked = false;
                sess.revoked_reason = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const W: Address = address!("1111111111111111111111111111111111111111");
    const S: Address = address!("2222222222222222222222222222222222222222");
    const T: Address = address!("3333333333333333333333333333333333333333");

    #[tokio::test]
    async fn revoking_blocks_future_signatures_and_reports_what_was_outstanding() {
        let s = Sessions::default();
        s.upsert("0xc1", W, S, T, 8453, 10_000, 100, 4.0).await;
        s.upsert("0xc1", W, S, T, 8453, 20_000, 200, 4.0).await;

        let stopped = s.revoke(W, S, "wallet_drainer").await;
        assert_eq!(stopped.vouchers, 2);
        assert_eq!(stopped.value, 20_000);
        assert_eq!(stopped.score_before, Some(4.0));
        assert!(s.is_revoked(W, S).await);
        assert!(s.open_pairs().await.is_empty(), "a revoked seller is no longer watched");
    }

    #[tokio::test]
    async fn a_seller_can_be_blocked_before_it_is_ever_paid() {
        let s = Sessions::default();
        let stopped = s.revoke(W, S, "operator").await;
        assert_eq!(stopped, Stopped::default());
        assert!(s.is_revoked(W, S).await);
        assert_eq!(s.blocked_for(W).await, vec![format!("{S:#x}")]);
    }

    #[tokio::test]
    async fn revocation_is_per_wallet() {
        let s = Sessions::default();
        let other = address!("4444444444444444444444444444444444444444");
        s.revoke(W, S, "x").await;
        assert!(!s.is_revoked(other, S).await);
    }

    #[test]
    fn a_person_approves_a_limit_not_one_voucher() {
        assert_eq!(approval_limit(51_000, 50_000), 100_000);
        assert_eq!(approval_limit(50_000, 50_000), 50_000, "an exact multiple is its own limit");
        assert_eq!(approval_limit(51_000, 0), 51_000, "no step: exactly what was asked");
    }

    #[tokio::test]
    async fn approved_limits_only_ever_rise() {
        let s = Sessions::default();
        assert_eq!(s.approved_limit("0xc1").await, 0);
        assert_eq!(s.raise_limit("0xc1", 100_000).await, 0);
        assert_eq!(s.raise_limit("0xc1", 50_000).await, 100_000);
        assert_eq!(s.approved_limit("0xc1").await, 100_000);
        assert_eq!(s.approved_limit("0xc2").await, 0, "limits are per channel");
    }

    #[tokio::test]
    async fn restore_reopens_the_seller() {
        let s = Sessions::default();
        s.upsert("0xc1", W, S, T, 8453, 10_000, 100, 4.0).await;
        s.revoke(W, S, "x").await;
        s.restore(W, S).await;
        assert!(!s.is_revoked(W, S).await);
        assert_eq!(s.open_pairs().await, vec![(W, S)]);
    }
}
