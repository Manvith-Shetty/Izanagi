//! Tab's durable state: accounts, sessions, tabs and receipts.
//!
//! One JSON file, rewritten atomically (write, then rename) on every change. Small by design:
//! the chain holds the money and the countersigner holds the decisions; this holds only what
//! Tab needs to remember between requests -- who is who, and what each seller has charged.

use alloy::primitives::{Address, B256};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use tokio::sync::RwLock;

const RECEIPTS_KEPT: usize = 200;

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// A random, URL-safe secret with a readable prefix: `tok_…`, `ses_…`, `acct_…`.
pub fn secret(prefix: &str) -> String {
    let a = alloy::signers::local::PrivateKeySigner::random().to_bytes();
    let hex: String = a.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{hex}")
}

/// One person: a verified human with their own wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    /// World ID fingerprint from the countersigner: stable for the person, private to Tab,
    /// never their identity. One account per fingerprint is the "one free tab per human" rule.
    pub human: String,
    pub wallet: Address,
    pub created_at: u64,
    /// The secret in this person's MCP link. Never shown to anyone else.
    #[serde(default)]
    pub mcp_token: String,
    pub trial_amount: u128,
    #[serde(default)]
    pub deploy_tx: Option<String>,
    #[serde(default)]
    pub fund_tx: Option<String>,
}

/// One tab: a channel from a person's wallet to one seller, on that seller's terms.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabRecord {
    pub channel_id: B256,
    pub account: String,
    pub seller: Address,
    pub receiver_authorizer: Address,
    pub token: Address,
    pub withdraw_delay: u64,
    pub salt: B256,
    /// Human-facing name for the seller, from its 402 or our catalog.
    #[serde(default)]
    pub service: Option<String>,
    pub price: u128,
    /// What the seller has charged us so far (cumulative).
    pub charged: u128,
    /// The highest voucher we have signed (cumulative).
    pub signed: u128,
    /// When the newest voucher's countersignature lapses.
    pub expiry: u64,
    pub requests: u64,
    pub deposited: u128,
    pub opened_at: u64,
    #[serde(default)]
    pub open_tx: Option<String>,
    /// What the seller had claimed on chain when we last looked.
    #[serde(default)]
    pub claimed: u128,
}

impl TabRecord {
    pub fn config(&self, wallet: Address) -> common::ChannelConfig {
        common::ChannelConfig::countersign(wallet, self.seller, self.receiver_authorizer, self.token, self.withdraw_delay, self.salt)
    }
}

/// One paid call, for the dashboard's history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub at: u64,
    pub account: String,
    pub seller: Address,
    #[serde(default)]
    pub service: Option<String>,
    pub url: String,
    pub price: u128,
    pub cumulative: u128,
    pub verdict: String,
    pub toxic_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Session {
    account: String,
    expires: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    accounts: BTreeMap<String, Account>,
    #[serde(default)]
    sessions: BTreeMap<String, Session>,
    #[serde(default)]
    tabs: BTreeMap<String, TabRecord>,
    #[serde(default)]
    receipts: VecDeque<Receipt>,
}

pub struct Store {
    path: Option<PathBuf>,
    inner: RwLock<Persisted>,
}

const SESSION_TTL: u64 = 7 * 24 * 3600;

impl Store {
    pub fn ephemeral() -> Self {
        Self { path: None, inner: RwLock::new(Persisted::default()) }
    }

    pub fn load(path: PathBuf) -> Result<Self> {
        let inner = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Persisted::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self { path: Some(path), inner: RwLock::new(inner) })
    }

    fn save(&self, p: &Persisted) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(p)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /* -------------------------------- accounts -------------------------------- */

    pub async fn account_by_human(&self, human: &str) -> Option<Account> {
        self.inner.read().await.accounts.values().find(|a| a.human == human).cloned()
    }

    pub async fn account(&self, id: &str) -> Option<Account> {
        self.inner.read().await.accounts.get(id).cloned()
    }

    pub async fn account_by_mcp_token(&self, token: &str) -> Option<Account> {
        if token.len() < 16 {
            return None;
        }
        self.inner.read().await.accounts.values().find(|a| a.mcp_token == token).cloned()
    }

    pub async fn account_count(&self) -> usize {
        self.inner.read().await.accounts.len()
    }

    pub async fn put_account(&self, a: Account) -> Result<()> {
        let mut g = self.inner.write().await;
        g.accounts.insert(a.id.clone(), a);
        self.save(&g)
    }

    /* -------------------------------- sessions -------------------------------- */

    pub async fn new_session(&self, account: &str) -> Result<String> {
        let token = secret("ses");
        let mut g = self.inner.write().await;
        let now = now();
        g.sessions.retain(|_, s| s.expires > now);
        g.sessions.insert(token.clone(), Session { account: account.to_string(), expires: now + SESSION_TTL });
        self.save(&g)?;
        Ok(token)
    }

    pub async fn session_account(&self, token: &str) -> Option<Account> {
        let g = self.inner.read().await;
        let s = g.sessions.get(token)?;
        if s.expires <= now() {
            return None;
        }
        g.accounts.get(&s.account).cloned()
    }

    pub async fn end_session(&self, token: &str) -> Result<()> {
        let mut g = self.inner.write().await;
        g.sessions.remove(token);
        self.save(&g)
    }

    /* ---------------------------------- tabs ---------------------------------- */

    pub async fn tab(&self, channel_id: B256) -> Option<TabRecord> {
        self.inner.read().await.tabs.get(&format!("{channel_id:#x}")).cloned()
    }

    pub async fn tabs_of(&self, account: &str) -> Vec<TabRecord> {
        let mut v: Vec<_> = self.inner.read().await.tabs.values().filter(|t| t.account == account).cloned().collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.opened_at));
        v
    }

    pub async fn all_tabs(&self) -> Vec<TabRecord> {
        self.inner.read().await.tabs.values().cloned().collect()
    }

    pub async fn put_tab(&self, t: TabRecord) -> Result<()> {
        let mut g = self.inner.write().await;
        g.tabs.insert(format!("{:#x}", t.channel_id), t);
        self.save(&g)
    }

    /* -------------------------------- receipts -------------------------------- */

    pub async fn add_receipt(&self, r: Receipt) -> Result<()> {
        let mut g = self.inner.write().await;
        if g.receipts.len() >= RECEIPTS_KEPT {
            g.receipts.pop_front();
        }
        g.receipts.push_back(r);
        self.save(&g)
    }

    pub async fn receipts_of(&self, account: &str, limit: usize) -> Vec<Receipt> {
        self.inner.read().await.receipts.iter().rev().filter(|r| r.account == account).take(limit).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn account(id: &str, human: &str) -> Account {
        Account {
            id: id.into(),
            human: human.into(),
            wallet: address!("1111111111111111111111111111111111111111"),
            created_at: 1,
            mcp_token: secret("tok"),
            trial_amount: 250_000,
            deploy_tx: None,
            fund_tx: None,
        }
    }

    #[tokio::test]
    async fn one_account_per_human_is_findable_by_fingerprint_and_token() {
        let s = Store::ephemeral();
        let a = account("acct_1", "human-aaaa");
        s.put_account(a.clone()).await.unwrap();
        assert_eq!(s.account_by_human("human-aaaa").await.unwrap().id, "acct_1");
        assert!(s.account_by_human("human-bbbb").await.is_none());
        assert_eq!(s.account_by_mcp_token(&a.mcp_token).await.unwrap().id, "acct_1");
        assert!(s.account_by_mcp_token("short").await.is_none(), "trivial tokens never match");
    }

    #[tokio::test]
    async fn sessions_resolve_to_their_account_and_end() {
        let s = Store::ephemeral();
        s.put_account(account("acct_1", "human-aaaa")).await.unwrap();
        let t = s.new_session("acct_1").await.unwrap();
        assert_eq!(s.session_account(&t).await.unwrap().id, "acct_1");
        s.end_session(&t).await.unwrap();
        assert!(s.session_account(&t).await.is_none());
        assert!(s.session_account("ses_forged").await.is_none());
    }

    #[tokio::test]
    async fn state_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("tab-store-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let s = Store::load(path.clone()).unwrap();
        s.put_account(account("acct_1", "human-aaaa")).await.unwrap();
        let t = s.new_session("acct_1").await.unwrap();
        let again = Store::load(path.clone()).unwrap();
        assert_eq!(again.session_account(&t).await.unwrap().human, "human-aaaa");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn secrets_are_long_prefixed_and_distinct() {
        let a = secret("tok");
        assert!(a.starts_with("tok_") && a.len() == 68);
        assert_ne!(a, secret("tok"));
    }
}
