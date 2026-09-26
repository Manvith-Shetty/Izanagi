//! Everything Tab asks of, or sends to, the chain.
//!
//! Two signing keys, each with the least it needs:
//!
//!   * the **agent** key opens tabs: it calls the escrow's `deposit`, and the collector moves
//!     the wallet's own funds -- within the allowance its owner granted -- into a channel whose
//!     payer is that same wallet. It cannot send funds anywhere else.
//!   * the **treasury** key deploys each person's Countersign wallet, owns it, and funds their
//!     trial. It is the "bring your own owner" slot, filled by Tab for the free trial.
//!
//! Public RPCs load-balance across nodes that can disagree for a moment about an account's next
//! nonce, and they rate-limit. So every read retries on a 429, and each key sends one
//! transaction at a time with its nonce tracked locally.

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, TxHash, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::client::ClientBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use alloy::transports::layers::RetryBackoffLayer;
use anyhow::{anyhow, Context, Result};
use common::escrow::{ICountersign, IX402BatchSettlement, IERC20};
use common::{ChannelConfig, ESCROW, USDC_BASE};
use serde::Serialize;
use tokio::sync::Mutex;

/// Creation code of `contracts/src/Countersign.sol:Countersign`, exported by
/// `scripts/export-bytecode.sh`. A test checks it still matches the compiled contract.
const COUNTERSIGN_BIN: &str = include_str!("../assets/Countersign.bin");

/// One tab (channel) as the escrow holds it.
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnChainTab {
    pub balance: u128,
    pub total_claimed: u128,
    /// Non-zero while the wallet's owner is taking money back out.
    pub withdraw_finalize_after: u64,
}

/// A person's wallet, as the chain describes it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletState {
    pub usdc: u128,
    /// What the collector may still move into new tabs.
    pub allowance: u128,
    pub paused: bool,
    pub owner: Address,
    pub risk_oracle: Address,
}

pub struct Chain {
    read: DynProvider,
    agent: Mutex<DynProvider>,
    treasury: Mutex<DynProvider>,
    pub agent_address: Address,
    pub treasury_address: Address,
    pub collector: Address,
}

fn client(rpc: &str) -> Result<alloy::rpc::client::RpcClient> {
    // up to 8 retries on a rate limit, starting at 800ms; public Base RPC is ~a few req/s
    let url = rpc.parse().context("BASE_RPC is not a URL")?;
    Ok(ClientBuilder::default().layer(RetryBackoffLayer::new(8, 800, 100)).http(url))
}

fn signing(rpc: &str, key: &str, name: &str) -> Result<(DynProvider, Address)> {
    let signer: PrivateKeySigner = key.trim().trim_start_matches("0x").parse()
        .with_context(|| format!("{name} is not a private key"))?;
    let address = signer.address();
    let p = ProviderBuilder::new()
        .with_cached_nonce_management()
        .wallet(EthereumWallet::from(signer))
        .connect_client(client(rpc)?)
        .erased();
    Ok((p, address))
}

impl Chain {
    pub fn new(rpc: &str, agent_key: &str, treasury_key: &str, collector: Address) -> Result<Self> {
        let read = ProviderBuilder::new().connect_client(client(rpc)?).erased();
        let (agent, agent_address) = signing(rpc, agent_key, "AGENT_PRIVATE_KEY")?;
        let (treasury, treasury_address) = signing(rpc, treasury_key, "TAB_TREASURY_KEY")?;
        Ok(Self {
            read,
            agent: Mutex::new(agent),
            treasury: Mutex::new(treasury),
            agent_address,
            treasury_address,
            collector,
        })
    }

    /* ---------------------------------- reads ---------------------------------- */

    pub async fn tab(&self, channel_id: alloy::primitives::B256) -> Result<OnChainTab> {
        let escrow = IX402BatchSettlement::new(ESCROW, &self.read);
        let c = escrow.channels(channel_id).call().await.context("escrow.channels")?;
        let w = escrow.pendingWithdrawals(channel_id).call().await.context("escrow.pendingWithdrawals")?;
        Ok(OnChainTab {
            balance: c.balance,
            total_claimed: c.totalClaimed,
            withdraw_finalize_after: w.finalizeAfter.to::<u64>(),
        })
    }

    pub async fn wallet(&self, wallet: Address) -> Result<WalletState> {
        let usdc = IERC20::new(USDC_BASE, &self.read);
        let w = ICountersign::new(wallet, &self.read);
        Ok(WalletState {
            usdc: usdc.balanceOf(wallet).call().await.context("USDC balance")?.to::<u128>(),
            allowance: usdc.allowance(wallet, self.collector).call().await.context("allowance")?.to::<u128>(),
            paused: w.paused().call().await.context("paused")?,
            owner: w.owner().call().await.context("owner")?,
            risk_oracle: w.riskOracle().call().await.context("riskOracle")?,
        })
    }

    /// Is this seller revoked by this wallet, right now?
    pub async fn revoked(&self, wallet: Address, seller: Address) -> Result<bool> {
        Ok(ICountersign::new(wallet, &self.read).revocations(seller).call().await.context("revocations")?.revoked)
    }

    pub async fn gas_balance(&self, who: Address) -> Result<U256> {
        self.read.get_balance(who).await.context("ETH balance")
    }

    /* --------------------------------- the agent --------------------------------- */

    /// Open (or top up) a tab: the escrow pulls `amount` of the wallet's own USDC, through the
    /// collector, into the channel `cfg`. The wallet's owner granted the allowance for this.
    pub async fn open_tab(&self, cfg: &ChannelConfig, amount: u128) -> Result<TxHash> {
        let p = self.agent.lock().await;
        let escrow = IX402BatchSettlement::new(ESCROW, &*p);
        let receipt = escrow
            .deposit(cfg.into(), amount, self.collector, Bytes::new())
            .send()
            .await
            .context("sending the deposit")?
            .get_receipt()
            .await
            .context("waiting for the deposit receipt")?;
        if !receipt.status() {
            return Err(anyhow!("deposit {} reverted", receipt.transaction_hash));
        }
        Ok(receipt.transaction_hash)
    }

    /* -------------------------------- the treasury -------------------------------- */

    /// Deploy a Countersign wallet: owned by the treasury, spent by the agent key, guarded by
    /// the countersigner's oracle.
    pub async fn deploy_wallet(&self, oracle: Address) -> Result<(Address, TxHash)> {
        let code = hex(COUNTERSIGN_BIN.trim())?;
        let args = (self.treasury_address, ESCROW, self.agent_address, oracle).abi_encode_params();
        let init: Bytes = [code, args].concat().into();
        let p = self.treasury.lock().await;
        let tx = TransactionRequest::default().with_deploy_code(init);
        let receipt = p
            .send_transaction(tx)
            .await
            .context("sending the wallet deployment")?
            .get_receipt()
            .await
            .context("waiting for the deployment receipt")?;
        if !receipt.status() {
            return Err(anyhow!("deployment {} reverted", receipt.transaction_hash));
        }
        let wallet = receipt.contract_address.ok_or_else(|| anyhow!("deployment receipt has no contract address"))?;
        Ok((wallet, receipt.transaction_hash))
    }

    /// Give a new wallet its trial: `amount` of USDC, and an allowance for its collector.
    pub async fn fund_wallet(&self, wallet: Address, amount: u128) -> Result<(TxHash, TxHash)> {
        let p = self.treasury.lock().await;
        let usdc = IERC20Write::new(USDC_BASE, &*p);
        let t1 = usdc.transfer(wallet, U256::from(amount)).send().await.context("sending the trial USDC")?
            .get_receipt().await.context("waiting for the transfer")?;
        if !t1.status() {
            return Err(anyhow!("trial transfer {} reverted", t1.transaction_hash));
        }
        let w = ICountersignOwner::new(wallet, &*p);
        let t2 = w.approveToken(USDC_BASE, self.collector, U256::from(amount)).send().await
            .context("sending the collector allowance")?
            .get_receipt().await.context("waiting for the allowance")?;
        if !t2.status() {
            return Err(anyhow!("allowance {} reverted", t2.transaction_hash));
        }
        Ok((t1.transaction_hash, t2.transaction_hash))
    }
}

alloy::sol! {
    #[sol(rpc)]
    interface IERC20Write {
        function transfer(address to, uint256 amount) external returns (bool);
    }
    #[sol(rpc)]
    interface ICountersignOwner {
        function approveToken(address token, address spender, uint256 amount) external;
    }
}

fn hex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim_start_matches("0x");
    if s.len() % 2 != 0 {
        return Err(anyhow!("odd-length bytecode"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow!("bad bytecode hex: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_bytecode_decodes() {
        let code = hex(COUNTERSIGN_BIN.trim()).unwrap();
        assert!(code.len() > 3000, "creation code is suspiciously small: {}", code.len());
    }

    /// The copy Tab deploys must be the contract in this repo, compiled. If this fails, run
    /// scripts/export-bytecode.sh.
    #[test]
    fn the_embedded_bytecode_matches_the_compiled_contract() {
        let artifact = concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/out/Countersign.sol/Countersign.json");
        let Ok(json) = std::fs::read_to_string(artifact) else {
            eprintln!("no forge artifact at {artifact}; skipping (run forge build to check)");
            return;
        };
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["bytecode"]["object"].as_str().unwrap().trim(),
            COUNTERSIGN_BIN.trim(),
            "tab/assets/Countersign.bin is stale: run scripts/export-bytecode.sh"
        );
    }
}
