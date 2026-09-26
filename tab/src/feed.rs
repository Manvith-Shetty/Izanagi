//! One live feed for the product: what the countersigner decided, and what Tab did about it.
//!
//! The countersigner's journal says *screened, countersigned, asked, approved, revoked*. Only
//! Tab sees the rest of the story -- a tab opened on chain, a call paid for, a seller cashing
//! vouchers in. The dashboard and the approval page watch this merged feed, so the whole
//! sequence reads in order, live.

use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, RwLock};

const RETAINED: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Decided by the countersigner.
    Brain,
    /// Done by Tab.
    Tab,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: u64,
    pub at: u64,
    pub source: Source,
    pub kind: String,
    /// The event's own fields, flattened.
    #[serde(flatten)]
    pub data: serde_json::Map<String, Value>,
}

pub struct Feed {
    next: AtomicU64,
    recent: RwLock<VecDeque<Item>>,
    live: broadcast::Sender<Item>,
}

impl Default for Feed {
    fn default() -> Self {
        Self { next: AtomicU64::new(1), recent: RwLock::new(VecDeque::new()), live: broadcast::channel(256).0 }
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

impl Feed {
    /// Something Tab did.
    pub async fn tab(&self, kind: &str, data: Value) -> Item {
        let data = match data {
            Value::Object(m) => m,
            other => serde_json::Map::from_iter([("value".to_string(), other)]),
        };
        self.push(Source::Tab, kind.to_string(), now(), data).await
    }

    /// A countersigner journal entry, relayed as is.
    pub async fn brain(&self, entry: Value) -> Option<Item> {
        let Value::Object(mut m) = entry else { return None };
        let kind = m.remove("kind")?.as_str()?.to_string();
        let at = m.remove("at").and_then(|v| v.as_u64()).unwrap_or_else(now);
        // keep the countersigner's own id, so a reconnect can resume from it
        if let Some(id) = m.remove("id") {
            m.insert("brainId".into(), id);
        }
        Some(self.push(Source::Brain, kind, at, m).await)
    }

    async fn push(&self, source: Source, kind: String, at: u64, data: serde_json::Map<String, Value>) -> Item {
        let item = Item { id: self.next.fetch_add(1, Ordering::Relaxed), at, source, kind, data };
        {
            let mut r = self.recent.write().await;
            if r.len() == RETAINED {
                r.pop_front();
            }
            r.push_back(item.clone());
        }
        let _ = self.live.send(item.clone());
        item
    }

    pub async fn since(&self, after: u64) -> Vec<Item> {
        self.recent.read().await.iter().filter(|i| i.id > after).cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Item> {
        self.live.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn brain_entries_keep_their_kind_time_and_id() {
        let f = Feed::default();
        let i = f
            .brain(json!({"id": 41, "at": 1000, "kind": "revoked", "seller": "0xabc"}))
            .await
            .unwrap();
        assert_eq!(i.kind, "revoked");
        assert_eq!(i.at, 1000);
        assert_eq!(i.source, Source::Brain);
        assert_eq!(i.data["brainId"], 41);
        let v = serde_json::to_value(&i).unwrap();
        assert_eq!(v["seller"], "0xabc", "fields are flattened for the UI");
        assert_eq!(v["source"], "brain");
    }

    #[tokio::test]
    async fn entries_without_a_kind_are_dropped() {
        assert!(Feed::default().brain(json!({"id": 1})).await.is_none());
        assert!(Feed::default().brain(json!("text")).await.is_none());
    }

    #[tokio::test]
    async fn tab_items_and_brain_items_share_one_ordering() {
        let f = Feed::default();
        let a = f.tab("purchase", json!({"price": 1000})).await;
        let b = f.brain(json!({"kind": "countersigned"})).await.unwrap();
        assert!(b.id > a.id);
        assert_eq!(f.since(a.id).await.len(), 1);
    }
}
