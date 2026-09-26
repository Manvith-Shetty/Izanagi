//! Everything Tab asks of, or sends to, the chain.
//!
//! Two signing keys, each with the least it needs:
//!
//!   * the **agent** key opens tabs: it asks the wallet to `openTab`, which approves the collector
//!     for exactly that deposit and moves the wallet's own funds into a channel whose payer is
//!     that same wallet. It cannot send funds anywhere else, and no allowance is left standing.
//!
//! Tab holds no key that owns or funds anyone's wallet. A person's own wallet app (MetaMask)
//! creates their Countersign wallet, owns it from the first block, and adds its money; Tab only
//! builds those transactions, and checks on chain that they did what they should.
//!
//! Public RPCs load-balance across nodes that can disagree for a moment about an account's next
//! nonce, and they rate-limit. So every read retries on a 429, and each key sends one
//! transaction at a time with its nonce tracked locally.

use alloy::consensus::Transaction as _;
use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, TxHash, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::client::ClientBuilder;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::{SolCall, SolValue};
use alloy::transports::layers::RetryBackoffLayer;
use anyhow::{anyhow, Context, Result};
use common::escrow::{ICountersign, IX402BatchSettlement, IERC20};
use common::{ChannelConfig, ESCROW, USDC_BASE};
use serde::Serialize;
use std::time::Duration;
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
    /// When the owner started taking money back out; zero if they have not. The escrow counts
    /// the channel's `withdrawDelay` from here: finishing any earlier reverts
    /// (`WithdrawDelayNotElapsed`, seen on Base mainnet).
    pub withdraw_started_at: u64,
}

impl OnChainTab {
    /// When a withdrawal the owner started can be finished, given the channel's delay.
    pub fn withdraw_ready_at(&self, withdraw_delay: u64) -> Option<u64> {
        (self.withdraw_started_at != 0).then(|| self.withdraw_started_at + withdraw_delay)
    }
}

/// A person's wallet, as the chain describes it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletState {
    pub usdc: u128,
    pub paused: bool,
    pub owner: Address,
    pub risk_oracle: Address,
}

pub struct Chain {
    read: DynProvider,
    agent: Mutex<DynProvider>,
    pub agent_address: Address,
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
    pub fn new(rpc: &str, agent_key: &str, collector: Address) -> Result<Self> {
        let read = ProviderBuilder::new().connect_client(client(rpc)?).erased();
        let (agent, agent_address) = signing(rpc, agent_key, "AGENT_PRIVATE_KEY")?;
        Ok(Self {
            read,
            agent: Mutex::new(agent),
            agent_address,
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
            withdraw_started_at: w.finalizeAfter.to::<u64>(),
        })
    }

    pub async fn wallet(&self, wallet: Address) -> Result<WalletState> {
        let usdc = IERC20::new(USDC_BASE, &self.read);
        let w = ICountersign::new(wallet, &self.read);
        Ok(WalletState {
            usdc: usdc.balanceOf(wallet).call().await.context("USDC balance")?.to::<u128>(),
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

    /// Open (or top up) a tab: the wallet moves `amount` of its own USDC, through the collector,
    /// into the channel `cfg`, approving the collector for exactly that and nothing after.
    pub async fn open_tab(&self, cfg: &ChannelConfig, amount: u128) -> Result<TxHash> {
        let p = self.agent.lock().await;
        let wallet = ICountersign::new(cfg.payer, &*p);
        let receipt = wallet
            .openTab(cfg.into(), amount)
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

    /* ------------------------------ a person's wallet ------------------------------ */

    /// The creation code of a person's wallet: owned by `owner` from the first block, spent by
    /// the agent key, guarded by the countersigner's `oracle`, opening tabs through our collector.
    pub fn wallet_initcode(&self, owner: Address, oracle: Address) -> Result<Bytes> {
        let code = hex(COUNTERSIGN_BIN.trim())?;
        let args = (owner, ESCROW, self.agent_address, oracle, self.collector).abi_encode_params();
        Ok([code, args].concat().into())
    }

    /// The wallet a person's transaction created, once it has landed. Accepted only if `owner`
    /// sent it and it deployed exactly the wallet `wallet_initcode(owner, oracle)` describes, so
    /// nobody can claim a wallet somebody else created, or one wired to other keys.
    pub async fn created_wallet(&self, tx: TxHash, owner: Address, oracle: Address, wait: Duration) -> Result<Address> {
        let receipt = self.landed(tx, wait).await?;
        if receipt.from != owner {
            return Err(anyhow!("{tx:#x} was sent by {:#x}, not by the account you connected ({owner:#x})", receipt.from));
        }
        let wallet = receipt.contract_address.ok_or_else(|| anyhow!("{tx:#x} created no contract"))?;
        let sent = self
            .read
            .get_transaction_by_hash(tx)
            .await
            .context("reading the transaction")?
            .ok_or_else(|| anyhow!("{tx:#x} not found"))?;
        if sent.input() != &self.wallet_initcode(owner, oracle)? {
            return Err(anyhow!("{tx:#x} did not create a Tab wallet: its code or keys differ"));
        }
        Ok(wallet)
    }

    /// A transaction's receipt once it has landed, and succeeded.
    async fn landed(&self, tx: TxHash, wait: Duration) -> Result<alloy::rpc::types::TransactionReceipt> {
        let deadline = tokio::time::Instant::now() + wait;
        let receipt = loop {
            if let Some(r) = self.read.get_transaction_receipt(tx).await.context("reading the receipt")? {
                break r;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!("{tx:#x} has not landed yet; try again in a minute"));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        if !receipt.status() {
            return Err(anyhow!("{tx:#x} failed on chain"));
        }
        Ok(receipt)
    }

    /// The USDC a transaction sent to `wallet`, and who sent it: the account that signed the
    /// transaction and whose own USDC moved. Waits for the transaction to land.
    pub async fn deposit_into(&self, wallet: Address, tx: TxHash, wait: Duration) -> Result<Deposit> {
        let receipt = self.landed(tx, wait).await?;
        let amount: U256 = receipt
            .inner
            .logs()
            .iter()
            .filter(|l| l.address() == USDC_BASE)
            .filter_map(|l| l.log_decode::<IERC20Write::Transfer>().ok())
            .map(|l| l.inner.data)
            .filter(|t| t.to == wallet && t.from == receipt.from)
            .map(|t| t.value)
            .sum();
        if amount.is_zero() {
            return Err(anyhow!("{tx:#x} sent no USDC from its signer to this wallet"));
        }
        Ok(Deposit { from: receipt.from, amount: amount.to::<u128>(), tx })
    }
}

/// Money a person put into their own wallet.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deposit {
    pub from: Address,
    pub amount: u128,
    pub tx: TxHash,
}

/// A transaction for the person's own wallet app (MetaMask) to sign and send. Tab builds the
/// calldata so the page never encodes anything itself.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerTx {
    /// Absent when the transaction creates a contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Address>,
    pub data: Bytes,
    pub label: String,
}

impl OwnerTx {
    /// Create the person's wallet from its creation code (`Chain::wallet_initcode`).
    pub fn create_wallet(initcode: Bytes) -> Self {
        Self { to: None, data: initcode, label: "Create your Tab wallet".into() }
    }

    /// Send `amount` of USDC from the person's account to their Tab wallet.
    pub fn deposit(wallet: Address, amount: u128) -> Self {
        let data = IERC20Write::transferCall { to: wallet, amount: U256::from(amount) }.abi_encode();
        Self { to: Some(USDC_BASE), data: data.into(), label: "Add money to your Tab".into() }
    }

    /// Everything the owner signs to take their money out: start withdrawing each tab that still
    /// holds some, finish those whose wait is over, and sweep what sits in the wallet itself.
    pub fn withdraw(wallet: Address, owner: Address, idle: u128, tabs: &[(ChannelConfig, OnChainTab, String)], now: u64) -> Vec<Self> {
        let mut txs = Vec::new();
        for (cfg, on, name) in tabs {
            let left = on.balance.saturating_sub(on.total_claimed);
            if let Some(ready) = on.withdraw_ready_at(cfg.withdrawDelay.to::<u64>()) {
                if now >= ready {
                    let data = ICountersign::finalizeWithdrawCall { cfg: cfg.into() }.abi_encode();
                    txs.push(Self { to: Some(wallet), data: data.into(), label: format!("Take back what's left in your tab with {name}") });
                }
            } else if left > 0 {
                let data = ICountersign::initiateWithdrawCall { cfg: cfg.into(), amount: left }.abi_encode();
                txs.push(Self { to: Some(wallet), data: data.into(), label: format!("Start taking back your tab with {name}") });
            }
        }
        if idle > 0 {
            let data = ICountersign::sweepCall { token: USDC_BASE, to: owner, amount: U256::from(idle) }.abi_encode();
            txs.push(Self { to: Some(wallet), data: data.into(), label: "Send your wallet's balance to you".into() });
        }
        txs
    }
}

alloy::sol! {
    #[sol(rpc)]
    interface IERC20Write {
        event Transfer(address indexed from, address indexed to, uint256 value);
        function transfer(address to, uint256 amount) external returns (bool);
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

    fn cfg(wallet: Address, salt: u8) -> ChannelConfig {
        ChannelConfig::countersign(wallet, Address::repeat_byte(0x5e), Address::repeat_byte(0xa1), USDC_BASE, 86400, alloy::primitives::B256::repeat_byte(salt))
    }

    #[test]
    fn a_deposit_is_a_usdc_transfer_to_the_wallet() {
        let wallet = Address::repeat_byte(0x77);
        let tx = OwnerTx::deposit(wallet, 5_000_000);
        assert_eq!(tx.to, Some(USDC_BASE));
        let call = IERC20Write::transferCall::abi_decode(&tx.data).unwrap();
        assert_eq!((call.to, call.amount), (wallet, U256::from(5_000_000u64)));
    }

    #[test]
    fn taking_money_out_starts_finishes_and_sweeps() {
        // every tab here has a one-day withdraw delay, counted from when the withdrawal started
        let (wallet, owner, now) = (Address::repeat_byte(0x77), Address::repeat_byte(0xb0), 200_000);
        let tab = |balance, claimed, started| OnChainTab { balance, total_claimed: claimed, withdraw_started_at: started };
        let tabs = vec![
            (cfg(wallet, 1), tab(50_000, 20_000, 0), "open".to_string()),         // start: 30_000 left
            (cfg(wallet, 2), tab(50_000, 50_000, 0), "spent".to_string()),        // nothing left: skipped
            (cfg(wallet, 3), tab(50_000, 0, 100_000), "ready".to_string()),       // a day has passed: finish
            (cfg(wallet, 4), tab(50_000, 0, 150_000), "waiting".to_string()),     // started a moment ago: wait
        ];
        let txs = OwnerTx::withdraw(wallet, owner, 70_000, &tabs, now);
        assert_eq!(txs.len(), 3, "{txs:#?}");
        assert!(txs.iter().all(|t| t.to == Some(wallet)));

        let start = ICountersign::initiateWithdrawCall::abi_decode(&txs[0].data).unwrap();
        assert_eq!(start.amount, 30_000);
        assert_eq!(start.cfg.salt, alloy::primitives::B256::repeat_byte(1));
        let finish = ICountersign::finalizeWithdrawCall::abi_decode(&txs[1].data).unwrap();
        assert_eq!(finish.cfg.salt, alloy::primitives::B256::repeat_byte(3));
        let sweep = ICountersign::sweepCall::abi_decode(&txs[2].data).unwrap();
        assert_eq!((sweep.token, sweep.to, sweep.amount), (USDC_BASE, owner, U256::from(70_000u64)));
    }

    #[test]
    fn nothing_to_take_out_means_nothing_to_sign() {
        assert!(OwnerTx::withdraw(Address::ZERO, Address::ZERO, 0, &[], 0).is_empty());
    }

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
