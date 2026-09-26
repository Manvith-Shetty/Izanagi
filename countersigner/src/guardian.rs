//! The kill switch, on chain.
//!
//! A Countersign wallet names this service's key as its `riskOracle`, which makes it a
//! guardian: it may `revoke` and `restore` sellers and pause the wallet. That is the whole
//! of its on-chain power. It cannot move funds, open channels, or change who the agent is.
//!
//! Without a guardian the countersigner still protects the wallet -- it stops countersigning
//! and every outstanding attestation lapses within `ATTESTATION_TTL` -- but a revocation only
//! becomes instant, and visible to the seller, once it is written to the wallet.

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, TxHash};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use anyhow::{anyhow, Context, Result};
use common::escrow::ICountersign;
use serde::Serialize;

/// A Countersign wallet's standing towards one seller, read from the wallet itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnChainStanding {
    pub revoked: bool,
    pub reason: u32,
    pub revoked_at: u64,
    pub paused: bool,
}

#[derive(Clone)]
pub struct Guardian {
    provider: DynProvider,
    oracle: Address,
}

impl Guardian {
    pub fn new(rpc: &str, oracle_key: &str) -> Result<Self> {
        let signer: PrivateKeySigner = oracle_key.trim().trim_start_matches("0x").parse()
            .context("ORACLE_PRIVATE_KEY is not a private key")?;
        let oracle = signer.address();
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer))
            .connect_http(rpc.parse().context("BASE_RPC is not a URL")?)
            .erased();
        Ok(Self { provider, oracle })
    }

    pub fn oracle(&self) -> Address {
        self.oracle
    }

    /// Does this wallet actually name us as its guardian? Checked before sending anything,
    /// so a misconfigured wallet produces an explanation rather than a reverted transaction.
    pub async fn guards(&self, wallet: Address) -> Result<bool> {
        let oracle = ICountersign::new(wallet, &self.provider)
            .riskOracle()
            .call()
            .await
            .with_context(|| format!("{wallet:#x} is not a Countersign wallet"))?;
        Ok(oracle == self.oracle)
    }

    pub async fn standing(&self, wallet: Address, seller: Address) -> Result<OnChainStanding> {
        let w = ICountersign::new(wallet, &self.provider);
        let r = w.revocations(seller).call().await.context("reading revocations")?;
        let paused = w.paused().call().await.context("reading paused")?;
        Ok(OnChainStanding { revoked: r.revoked, reason: r.reason, revoked_at: r.revokedAt, paused })
    }

    /// Every voucher already signed for `seller` stops being claimable once this lands.
    pub async fn revoke(&self, wallet: Address, seller: Address, reason: u32) -> Result<TxHash> {
        self.ensure_guardian(wallet).await?;
        let w = ICountersign::new(wallet, &self.provider);
        let receipt = w.revoke(seller, reason).send().await.context("sending revoke")?
            .get_receipt().await.context("waiting for the revoke receipt")?;
        if !receipt.status() {
            return Err(anyhow!("revoke {} reverted", receipt.transaction_hash));
        }
        Ok(receipt.transaction_hash)
    }

    /// Only ever called after a fresh World ID verification: see `approvals.rs`.
    pub async fn restore(&self, wallet: Address, seller: Address) -> Result<TxHash> {
        self.ensure_guardian(wallet).await?;
        let w = ICountersign::new(wallet, &self.provider);
        let receipt = w.restore(seller).send().await.context("sending restore")?
            .get_receipt().await.context("waiting for the restore receipt")?;
        if !receipt.status() {
            return Err(anyhow!("restore {} reverted", receipt.transaction_hash));
        }
        Ok(receipt.transaction_hash)
    }

    async fn ensure_guardian(&self, wallet: Address) -> Result<()> {
        if !self.guards(wallet).await? {
            return Err(anyhow!(
                "wallet {wallet:#x} does not name {:#x} as its riskOracle, so it cannot be guarded from here",
                self.oracle
            ));
        }
        Ok(())
    }
}
