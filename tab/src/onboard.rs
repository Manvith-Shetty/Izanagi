//! Getting a Tab: prove you are a unique human, then create your own wallet.
//!
//!   1. `start`     asks the countersigner for a World ID enrolment and hands the person a link.
//!   2. `poll`      waits for them to verify. The countersigner then returns only a fingerprint
//!                  of the person and a one-time grant -- never who they are. A fingerprint Tab
//!                  has seen before signs that person back in: one Tab per human.
//!   3. `wallet_tx` takes a signature from the person's own account (MetaMask) over this signup,
//!                  and builds the transaction that creates their wallet, owned by that account
//!                  from the first block. Their wallet app signs it and pays for it.
//!   4. `created`   checks on chain that exactly that account created exactly that wallet, and
//!                  binds it (through the grant) to this person: from then on only their World
//!                  ID can approve its payments.
//!
//! Tab holds no key that owns or funds anyone's wallet. The signature in step 3 is what stops a
//! stranger from claiming a wallet someone else just created: only the account that signed for
//! this signup may create its wallet.

use crate::brain::Brain;
use crate::chain::{Chain, OwnerTx};
use crate::feed::Feed;
use crate::store::{now, secret, Account, Store};
use alloy::primitives::{Address, Signature, TxHash};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Stage {
    /// Waiting for the person to finish World ID.
    Verifying { user_code: String, world_url: String, expires_at: u64 },
    /// Verified. Their own wallet app signs `message` for this signup, on `chain_id`, then
    /// creates their wallet.
    CreateWallet { message: String, chain_id: u64 },
    /// Their wallet is being checked on chain and tied to them.
    Creating { step: String },
    /// Done. `account` is theirs; the API turns this into a session.
    Ready { account: String, returning: bool },
    Failed { reason: String },
}

/// Who a verified person is to Tab, until their wallet exists.
struct Verified {
    human: String,
    grant: String,
    /// The account that signed for this signup, once it has.
    owner: Option<Address>,
}

pub struct Onboarding {
    brain: Brain,
    chain: Arc<Chain>,
    store: Arc<Store>,
    feed: Arc<Feed>,
    chain_id: u64,
    signups: RwLock<HashMap<String, Stage>>,
    verified: RwLock<HashMap<String, Verified>>,
    /// Polls advance one at a time, so an enrolment is claimed exactly once.
    polling: tokio::sync::Mutex<()>,
}

/// What the person's account signs to say "this signup is mine". Bound to the signup and the
/// chain, so it cannot be replayed onto another signup.
pub fn claim_message(signup: &str, chain_id: u64) -> String {
    format!("Create my Tab wallet\n\nSignup: {signup}\nChain: {chain_id}")
}

impl Onboarding {
    pub fn new(brain: Brain, chain: Arc<Chain>, store: Arc<Store>, feed: Arc<Feed>, chain_id: u64) -> Self {
        Self {
            brain,
            chain,
            store,
            feed,
            chain_id,
            signups: RwLock::new(HashMap::new()),
            verified: RwLock::new(HashMap::new()),
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

    async fn set(&self, id: &str, s: Stage) -> Stage {
        self.signups.write().await.insert(id.into(), s.clone());
        s
    }

    /// Where a signup has got to, advancing it when the person has verified.
    pub async fn poll(&self, id: &str) -> Result<Stage> {
        let _one_at_a_time = self.polling.lock().await;
        let stage = self.signups.read().await.get(id).cloned().ok_or_else(|| anyhow!("unknown signup"))?;
        let Stage::Verifying { .. } = stage else { return Ok(stage) };

        // denied or expired at World: say so rather than wait forever
        if let Some(v) = self.brain.approval(id).await? {
            if matches!(v.status.as_str(), "denied" | "expired") {
                let reason = format!("World ID verification did not complete ({})", v.denied_reason.unwrap_or(v.status));
                return Ok(self.set(id, Stage::Failed { reason }).await);
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

        // one Tab per human: someone we know signs back in
        if let Some(existing) = self.store.account_by_human(&enrolled.human).await {
            return Ok(self.set(id, Stage::Ready { account: existing.id, returning: true }).await);
        }
        self.verified
            .write()
            .await
            .insert(id.into(), Verified { human: enrolled.human, grant: enrolled.grant, owner: None });
        Ok(self.set(id, self.create_wallet(id)).await)
    }

    /// The transaction that creates this person's wallet, owned by the account that signed
    /// `claim_message` for this signup.
    pub async fn wallet_tx(&self, id: &str, owner: Address, signature: &str) -> Result<OwnerTx> {
        let signed: Signature = signature.trim().parse().context("that is not a signature")?;
        let signer = signed
            .recover_address_from_msg(claim_message(id, self.chain_id))
            .context("could not read the signature")?;
        if signer != owner {
            return Err(anyhow!("the signature is from {signer:#x}, not from {owner:#x}"));
        }
        if !matches!(self.signups.read().await.get(id), Some(Stage::CreateWallet { .. })) {
            return Err(anyhow!("this signup is not waiting for a wallet"));
        }
        let mut verified = self.verified.write().await;
        let v = verified.get_mut(id).ok_or_else(|| anyhow!("verify with World ID first"))?;
        v.owner = Some(owner);
        drop(verified);
        Ok(OwnerTx::create_wallet(self.chain.wallet_initcode(owner, self.oracle().await?)?))
    }

    /// The person's account sent `tx`. If it created their wallet, tie it to them and finish.
    pub async fn created(&self, id: &str, tx: TxHash) -> Result<Stage> {
        {
            let mut signups = self.signups.write().await;
            match signups.get(id) {
                Some(Stage::CreateWallet { .. }) => {
                    signups.insert(id.into(), Stage::Creating { step: "checking your wallet on Base".into() });
                }
                Some(s @ (Stage::Creating { .. } | Stage::Ready { .. })) => return Ok(s.clone()),
                Some(_) => return Err(anyhow!("this signup is not waiting for a wallet")),
                None => return Err(anyhow!("unknown signup")),
            }
        }
        match self.finish(id, tx).await {
            Ok(account) => {
                self.verified.write().await.remove(id);
                Ok(self.set(id, Stage::Ready { account, returning: false }).await)
            }
            // nothing is bound yet: the person may try again with the right transaction
            Err(e) => {
                self.set(id, self.create_wallet(id)).await;
                Err(e)
            }
        }
    }

    async fn finish(&self, id: &str, tx: TxHash) -> Result<String> {
        let (human, grant, owner) = {
            let verified = self.verified.read().await;
            let v = verified.get(id).ok_or_else(|| anyhow!("verify with World ID first"))?;
            let owner = v.owner.ok_or_else(|| anyhow!("connect your wallet app and sign first"))?;
            (v.human.clone(), v.grant.clone(), owner)
        };
        let wallet = self.chain.created_wallet(tx, owner, self.oracle().await?, Duration::from_secs(60)).await?;
        tracing::info!(%wallet, %owner, tx = %tx, "a person created their wallet");

        self.set(id, Stage::Creating { step: "tying it to your World ID".into() }).await;
        self.brain.bind(&grant, wallet).await?;

        let account = Account {
            id: secret("acct")[..21].to_string(),
            human,
            wallet,
            created_at: now(),
            mcp_token: secret("tok"),
            deploy_tx: Some(format!("{tx:#x}")),
        };
        self.store.put_account(account.clone()).await?;
        self.feed
            .tab("account_created", json!({"wallet": wallet, "owner": owner, "deployTx": account.deploy_tx}))
            .await;
        Ok(account.id)
    }

    fn create_wallet(&self, id: &str) -> Stage {
        Stage::CreateWallet { message: claim_message(id, self.chain_id), chain_id: self.chain_id }
    }

    /// The countersigner's oracle: the one key every wallet's payments need a signature from.
    async fn oracle(&self) -> Result<Address> {
        self.brain.health().await?["oracle"]
            .as_str()
            .ok_or_else(|| anyhow!("the countersigner did not report its oracle"))?
            .parse()
            .context("the countersigner's oracle is not an address")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::{local::PrivateKeySigner, SignerSync};

    #[test]
    fn a_claim_signature_names_its_signer_and_nobody_else() {
        let me = PrivateKeySigner::random();
        let sig = me.sign_message_sync(claim_message("apr_1", 8453).as_bytes()).unwrap();
        let recovered = sig.recover_address_from_msg(claim_message("apr_1", 8453)).unwrap();
        assert_eq!(recovered, me.address());
        // the same signature says nothing about another signup, or another chain
        assert_ne!(sig.recover_address_from_msg(claim_message("apr_2", 8453)).unwrap(), me.address());
        assert_ne!(sig.recover_address_from_msg(claim_message("apr_1", 1)).unwrap(), me.address());
    }
}
