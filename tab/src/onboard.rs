//! Getting a Tab: prove you are a unique human, receive your own wallet.
//!
//!   1. `start`  asks the countersigner for a World ID enrolment and hands the person a link.
//!   2. `poll`   waits for them to verify. The countersigner then returns only a fingerprint
//!               of the person and a one-time grant -- never who they are.
//!   3. A fingerprint Tab has seen before signs that person back in: one free tab per human.
//!      A new one gets a wallet deployed, funded with the trial amount, and bound (through the
//!      grant) to this person, so only their World ID can approve its payments from then on.
//!
//! Provisioning takes a few blocks, so it runs in the background and reports its stage.

use crate::brain::Brain;
use crate::chain::Chain;
use crate::feed::Feed;
use crate::store::{now, secret, Account, Store};
use alloy::primitives::Address;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Stage {
    /// Waiting for the person to finish World ID.
    Verifying { user_code: String, world_url: String, expires_at: u64 },
    /// Verified; their wallet is being made.
    Creating { step: String },
    /// Done. `account` is theirs; the API turns this into a session.
    Ready { account: String, returning: bool },
    Failed { reason: String },
}

pub struct Onboarding {
    brain: Brain,
    chain: Arc<Chain>,
    store: Arc<Store>,
    feed: Arc<Feed>,
    trial_amount: u128,
    max_accounts: usize,
    signups: RwLock<HashMap<String, Stage>>,
    /// Polls advance one at a time, so an enrolment is claimed exactly once.
    polling: tokio::sync::Mutex<()>,
}

impl Onboarding {
    pub fn new(brain: Brain, chain: Arc<Chain>, store: Arc<Store>, feed: Arc<Feed>, trial_amount: u128, max_accounts: usize) -> Self {
        Self {
            brain,
            chain,
            store,
            feed,
            trial_amount,
            max_accounts,
            signups: RwLock::new(HashMap::new()),
            polling: tokio::sync::Mutex::new(()),
        }
    }

    /// Begin: returns the signup id (the enrolment's approval id) and where to verify.
    pub async fn start(&self) -> Result<(String, Stage)> {
        let a = self.brain.enroll().await?;
        let stage = Stage::Verifying {
            user_code: a.user_code.clone(),
            world_url: a.world_link().to_string(),
            expires_at: a.expires_at,
        };
        self.signups.write().await.insert(a.id.clone(), stage.clone());
        Ok((a.id, stage))
    }

    /// Where a signup has got to, advancing it when the person has verified.
    pub async fn poll(self: &Arc<Self>, id: &str) -> Result<Stage> {
        let _one_at_a_time = self.polling.lock().await;
        let stage = self.signups.read().await.get(id).cloned().ok_or_else(|| anyhow!("unknown signup"))?;
        let Stage::Verifying { .. } = stage else { return Ok(stage) };

        // denied or expired at World: say so rather than wait forever
        if let Some(v) = self.brain.approval(id).await? {
            if matches!(v.status.as_str(), "denied" | "expired") {
                let s = Stage::Failed { reason: format!("World ID verification did not complete ({})", v.denied_reason.unwrap_or(v.status)) };
                self.signups.write().await.insert(id.into(), s.clone());
                return Ok(s);
            }
        }
        let enrolled = match self.brain.claim_enrollment(id).await {
            Ok(Some(e)) => e,
            Ok(None) => return Ok(stage),
            // a concurrent poll already claimed it and moved the signup on: report that stage
            Err(e) => match self.signups.read().await.get(id).cloned() {
                Some(s) if !matches!(s, Stage::Verifying { .. }) => return Ok(s),
                _ => return Err(e),
            },
        };

        // one tab per human: someone we know signs back in
        if let Some(existing) = self.store.account_by_human(&enrolled.human).await {
            let s = Stage::Ready { account: existing.id, returning: true };
            self.signups.write().await.insert(id.into(), s.clone());
            return Ok(s);
        }
        if self.store.account_count().await >= self.max_accounts {
            let s = Stage::Failed { reason: "all free trial tabs have been handed out".into() };
            self.signups.write().await.insert(id.into(), s.clone());
            return Ok(s);
        }

        let s = Stage::Creating { step: "deploying your wallet".into() };
        self.signups.write().await.insert(id.into(), s.clone());
        let me = self.clone();
        let (id, human, grant) = (id.to_string(), enrolled.human, enrolled.grant);
        tokio::spawn(async move {
            let s = match me.provision(&id, &human, &grant).await {
                Ok(account) => Stage::Ready { account, returning: false },
                Err(e) => {
                    tracing::error!(signup = %id, "provisioning failed: {e:#}");
                    Stage::Failed { reason: format!("could not create your wallet: {e:#}") }
                }
            };
            me.signups.write().await.insert(id, s);
        });
        Ok(s)
    }

    async fn step(&self, id: &str, step: &str) {
        self.signups.write().await.insert(id.into(), Stage::Creating { step: step.into() });
    }

    async fn provision(&self, id: &str, human: &str, grant: &str) -> Result<String> {
        let oracle: Address = self.brain.health().await?["oracle"]
            .as_str()
            .ok_or_else(|| anyhow!("the countersigner did not report its oracle"))?
            .parse()?;

        let (wallet, deploy_tx) = self.chain.deploy_wallet(oracle).await?;
        tracing::info!(%wallet, tx = %deploy_tx, "deployed a wallet");

        self.step(id, "binding it to your World ID").await;
        self.brain.bind(grant, wallet).await?;

        self.step(id, "adding your free trial funds").await;
        let (fund_tx, _) = self.chain.fund_wallet(wallet, self.trial_amount).await?;

        let account = Account {
            id: secret("acct")[..21].to_string(),
            human: human.into(),
            wallet,
            created_at: now(),
            mcp_token: secret("tok"),
            trial_amount: self.trial_amount,
            deploy_tx: Some(format!("{deploy_tx:#x}")),
            fund_tx: Some(format!("{fund_tx:#x}")),
        };
        self.store.put_account(account.clone()).await?;
        self.feed
            .tab("account_created", json!({"wallet": wallet, "trial": self.trial_amount,
                                            "deployTx": account.deploy_tx, "fundTx": account.fund_tx}))
            .await;
        Ok(account.id)
    }
}
