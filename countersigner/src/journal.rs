//! Everything the countersigner decides, as it decides it.
//!
//! One append-only feed with two readers: `GET /v1/activity` for whatever a client missed,
//! and `GET /v1/stream` (server-sent events) for what happens next. The dashboard, the approval
//! page and the MCP server all watch the same feed, so a human sees a revocation the moment
//! the watcher makes it, not the next time something polls.
//!
//! Entries never carry a credential: no device code, no OIDC subject, no signature.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, RwLock};

/// How many entries a late subscriber can catch up on.
const RETAINED: usize = 500;

/// Who flipped a kill switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// The re-screening watcher, because a live score crossed the refusal threshold.
    Watcher,
    /// The wallet's operator, from the dashboard or an agent's `close_tab`.
    Operator,
    /// A person, after a fresh World ID verification.
    Human,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A payment was screened. `verdict` is pay | cap | ask | refuse.
    Screened {
        wallet: String,
        seller: String,
        verdict: String,
        reason: String,
        toxic_score: f64,
        requested: u128,
    },
    /// A voucher ceiling was countersigned: the seller may claim up to `ceiling` until `expiry`.
    Countersigned {
        wallet: String,
        seller: String,
        channel_id: String,
        ceiling: u128,
        expiry: u64,
    },
    /// A person has been asked to approve something.
    ApprovalRequested {
        approval_id: String,
        purpose: String,
        wallet: String,
        seller: String,
        amount: u128,
        reason: String,
        expires_at: u64,
    },
    /// A verified person said yes. The subject stays in the countersigner.
    ApprovalGranted { approval_id: String, purpose: String, orb_verified: bool },
    /// The human path ended without a yes: denied, expired, wrong person, stale proof.
    ApprovalDenied { approval_id: String, purpose: String, reason: String },
    /// A wallet's approvals are now bound to one unique human.
    HumanBound { wallet: String, human: String },
    /// Every voucher already signed for this seller stopped being claimable.
    Revoked {
        wallet: String,
        seller: String,
        reason: String,
        reason_code: u32,
        by: Actor,
        /// Signed-but-unclaimed value that died at this instant.
        value_stopped: u128,
        vouchers_stopped: u64,
        score_before: Option<f64>,
        score_now: Option<f64>,
        /// The on-chain `revoke` transaction, when one was sent.
        tx: Option<String>,
        /// Set when the on-chain step failed. The in-memory revocation still stands, and the
        /// short attestation TTL retires every outstanding voucher on its own.
        chain_error: Option<String>,
    },
    /// A seller may be paid again. Only ever `by: human`.
    Restored { wallet: String, seller: String, by: Actor, tx: Option<String>, chain_error: Option<String> },
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: u64,
    pub at: u64,
    #[serde(flatten)]
    pub event: Event,
}

pub struct Journal {
    next: AtomicU64,
    recent: RwLock<VecDeque<Entry>>,
    live: broadcast::Sender<Entry>,
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            next: AtomicU64::new(1),
            recent: RwLock::new(VecDeque::with_capacity(RETAINED)),
            live: broadcast::channel(256).0,
        }
    }
}

impl Journal {
    pub async fn record(&self, event: Event) -> Entry {
        let entry = Entry { id: self.next.fetch_add(1, Ordering::Relaxed), at: crate::state::now(), event };
        {
            let mut r = self.recent.write().await;
            if r.len() == RETAINED {
                r.pop_front();
            }
            r.push_back(entry.clone());
        }
        // no subscribers is not an error: nobody is watching yet
        let _ = self.live.send(entry.clone());
        entry
    }

    /// Entries after `after` (exclusive), oldest first.
    pub async fn since(&self, after: u64) -> Vec<Entry> {
        self.recent.read().await.iter().filter(|e| e.id > after).cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Entry> {
        self.live.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn granted(n: u32) -> Event {
        Event::ApprovalGranted { approval_id: format!("apr_{n}"), purpose: "payment".into(), orb_verified: true }
    }

    #[tokio::test]
    async fn ids_are_increasing_and_since_is_exclusive() {
        let j = Journal::default();
        let a = j.record(granted(1)).await;
        let b = j.record(granted(2)).await;
        assert!(b.id > a.id);
        let after_a = j.since(a.id).await;
        assert_eq!(after_a.len(), 1);
        assert_eq!(after_a[0].id, b.id);
    }

    #[tokio::test]
    async fn retention_is_bounded() {
        let j = Journal::default();
        for n in 0..(RETAINED as u32 + 25) {
            j.record(granted(n)).await;
        }
        let all = j.since(0).await;
        assert_eq!(all.len(), RETAINED);
        assert_eq!(all.last().unwrap().id, RETAINED as u64 + 25);
    }

    #[tokio::test]
    async fn live_subscribers_see_new_entries() {
        let j = Journal::default();
        let mut rx = j.subscribe();
        j.record(granted(7)).await;
        let e = rx.recv().await.unwrap();
        assert!(matches!(e.event, Event::ApprovalGranted { .. }));
    }

    #[test]
    fn entries_serialize_flat_with_a_kind_tag() {
        let e = Entry { id: 3, at: 10, event: granted(1) };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "approval_granted");
        assert_eq!(v["id"], 3);
        assert_eq!(v["approval_id"], "apr_1");
    }
}
