//! Tab's state, and the operations the website and the MCP server share.

use crate::brain::Brain;
use crate::buyer::Buyer;
use crate::catalog::Catalog;
use crate::census::Censor;
use crate::chain::Chain;
use crate::env::TabEnv;
use crate::feed::Feed;
use crate::onboard::Onboarding;
use crate::store::{Account, Store};
use alloy::primitives::Address;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

pub struct App {
    pub env: TabEnv,
    pub brain: Brain,
    pub chain: Arc<Chain>,
    pub store: Arc<Store>,
    pub feed: Arc<Feed>,
    pub buyer: Buyer,
    pub onboarding: Arc<Onboarding>,
    pub catalog: Catalog,
    pub census: Arc<Censor>,
}

pub type Shared = Arc<App>;

fn now() -> u64 {
    crate::store::now()
}

impl App {
    pub fn new(env: TabEnv) -> Result<Shared> {
        std::fs::create_dir_all(&env.state_dir).ok();
        let brain = Brain::new(&env.countersigner_url, &env.control_token);
        let chain = Arc::new(Chain::new(&env.rpc, &env.agent_private_key, &env.treasury_key, env.collector)?);
        let store = Arc::new(Store::load(env.state_dir.join("tab.json"))?);
        let feed = Arc::new(Feed::default());
        let buyer = Buyer::new(
            &env.countersigner_url,
            &env.agent_private_key,
            brain.clone(),
            chain.clone(),
            store.clone(),
            feed.clone(),
            env.chain_id,
            env.tab_deposit,
            env.max_price,
        )?;
        let onboarding = Arc::new(Onboarding::new(
            brain.clone(),
            chain.clone(),
            store.clone(),
            feed.clone(),
            env.trial_amount,
            env.trial_max_accounts,
        ));
        let catalog = Catalog::new(env.chain_id, env.demo_shop_url.clone());
        let census = Arc::new(Censor::new(&env.blockscout_api, &env.census_rpc, env.state_dir.join("census.json")));
        Ok(Arc::new(Self { env, brain, chain, store, feed, buyer, onboarding, catalog, census }))
    }

    /// Everything a person sees about their own Tab.
    pub async fn overview(&self, a: &Account) -> Value {
        let wallet = self.chain.wallet(a.wallet).await.ok();
        let brain = self.brain.wallet(a.wallet).await.ok();
        let closed: HashSet<String> = brain
            .as_ref()
            .and_then(|b| b["closedSellers"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| s.as_str().map(str::to_lowercase))
            .collect();
        let scores: Vec<Value> = brain.as_ref().and_then(|b| b["sessions"].as_array().cloned()).unwrap_or_default();

        let mut tabs = Vec::new();
        let (mut spent, mut stoppable, mut escrowed) = (0u128, 0u128, 0u128);
        for t in self.store.tabs_of(&a.id).await {
            let on = self.chain.tab(t.channel_id).await.ok();
            let claimed = on.map(|o| o.total_claimed).unwrap_or(t.claimed);
            let balance = on.map(|o| o.balance).unwrap_or(0);
            let seller = format!("{:#x}", t.seller);
            let is_closed = closed.contains(&seller) || self.chain.revoked(a.wallet, t.seller).await.unwrap_or(false);
            let lapsed = t.expiry != 0 && now() > t.expiry;
            let open_value = t.charged.saturating_sub(claimed);
            let score = scores.iter().find(|s| s["seller"].as_str() == Some(seller.as_str())).and_then(|s| s["lastScore"].as_f64());
            spent += t.charged;
            escrowed += balance.saturating_sub(claimed);
            if !is_closed && !lapsed {
                stoppable += open_value;
            }
            tabs.push(json!({
                "channelId": t.channel_id,
                "seller": seller,
                "service": t.service,
                "price": t.price,
                "requests": t.requests,
                "charged": t.charged,
                "claimed": claimed,
                "stoppable": if is_closed || lapsed { 0 } else { open_value },
                "escrowed": balance.saturating_sub(claimed),
                "deposited": t.deposited,
                "expiry": t.expiry,
                "withdrawing": on.map(|o| o.withdraw_finalize_after != 0).unwrap_or(false),
                "status": if is_closed { "closed" } else if lapsed { "lapsed" } else { "open" },
                "riskScore": score,
                "openTx": t.open_tx,
            }));
        }

        json!({
            "account": {
                "id": a.id,
                "wallet": a.wallet,
                "human": brain.as_ref().and_then(|b| b["human"].as_str()).unwrap_or(&a.human),
                "createdAt": a.created_at,
                "trial": a.trial_amount,
                "deployTx": a.deploy_tx,
                "fundTx": a.fund_tx,
            },
            "wallet": wallet,
            "totals": {"spent": spent, "stoppable": stoppable, "escrowed": escrowed},
            "tabs": tabs,
            "approvals": brain.as_ref().map(|b| b["approvals"].clone()).unwrap_or(json!([])),
            "receipts": self.store.receipts_of(&a.id, 25).await,
            "mcpUrl": self.env.mcp_url(&a.mcp_token),
            "network": {"chainId": self.env.chain_id, "fork": self.env.is_fork()},
        })
    }

    /// Stop paying `seller`: always allowed, never needs a person.
    pub async fn close_tab(&self, a: &Account, seller: Address, reason: Option<&str>) -> Result<Value> {
        self.owns_tab(a, seller).await?;
        self.brain.revoke(a.wallet, seller, reason).await
    }

    /// Ask the person to reopen a closed tab. Nothing reopens until they approve.
    pub async fn reopen_tab(&self, a: &Account, seller: Address) -> Result<Value> {
        self.owns_tab(a, seller).await?;
        self.brain.restore(a.wallet, seller).await
    }

    async fn owns_tab(&self, a: &Account, seller: Address) -> Result<()> {
        if self.store.tabs_of(&a.id).await.iter().any(|t| t.seller == seller) {
            Ok(())
        } else {
            Err(anyhow!("you have no tab with {seller:#x}"))
        }
    }

    /// Notice when a seller cashes in, so "stoppable" turns into "final" on screen.
    pub async fn watch_claims(self: Arc<Self>) {
        loop {
            for mut t in self.store.all_tabs().await {
                let Ok(on) = self.chain.tab(t.channel_id).await else { continue };
                if on.total_claimed > t.claimed {
                    let newly = on.total_claimed - t.claimed;
                    t.claimed = on.total_claimed;
                    let seller = t.seller;
                    let wallet = self.store.account(&t.account).await.map(|a| a.wallet);
                    if self.store.put_tab(t.clone()).await.is_ok() {
                        self.feed
                            .tab("seller_claimed", json!({"wallet": wallet, "seller": seller, "service": t.service,
                                                            "amount": newly, "claimed": t.claimed, "channelId": t.channel_id}))
                            .await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }
}
