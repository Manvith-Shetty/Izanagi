//! Everything the seller asks the chain.
//!
//! The one call that matters is `voucher_valid`: for an EIP-1271 payer it asks the payer's own
//! contract the exact question the escrow will ask at claim time. For a Countersign payer that
//! answer can change after we served the request -- which is the whole point.

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, Signature, TxHash, B256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::client::ClientBuilder;
use alloy::transports::layers::RetryBackoffLayer;
use alloy::signers::local::PrivateKeySigner;
use anyhow::{anyhow, Context, Result};
use common::escrow::{ICountersign, IX402BatchSettlement, IERC1271, EIP1271_MAGIC};
use common::{ChannelConfig, ESCROW};

/// On-chain view of one channel.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OnChain {
    pub balance: u128,
    pub total_claimed: u128,
    /// Non-zero while the payer's timed withdrawal is pending.
    pub withdraw_finalize_after: u64,
}

/// What a Countersign payer's wallet says about us.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CountersignStatus {
    pub revoked: bool,
    /// Intercepta trait code the wallet recorded when it revoked us.
    pub reason: u32,
    pub revoked_at: u64,
    pub paused: bool,
}

/// A claim the escrow refused, with what it said.
#[derive(Debug, Clone)]
pub struct Rejected(pub String);

pub struct Chain {
    provider: DynProvider,
    /// The receiver authorizer: signs claims, sends them, pays the gas.
    authorizer: Address,
}

/// `Some(message)` when the node answered and the answer was "no" (a revert); `None` when we
/// never got an answer. The distinction keeps an RPC outage from reading as a bad voucher.
fn chain_said_no(e: &alloy::contract::Error) -> Option<String> {
    match e {
        alloy::contract::Error::TransportError(t) => t.as_error_resp().map(|r| {
            match e.as_revert_data() {
                Some(data) if !data.is_empty() => format!("{} (revert data {data})", r.message),
                _ => r.message.to_string(),
            }
        }),
        _ => None,
    }
}

impl Chain {
    pub fn new(rpc: &str, authorizer_key: &str) -> Result<Self> {
        let signer: PrivateKeySigner = authorizer_key.trim().trim_start_matches("0x").parse()
            .context("SELLER_AUTHORIZER_KEY is not a private key")?;
        let authorizer = signer.address();
        // public RPCs rate-limit (HTTP 429): retry with backoff so a busy endpoint does not
        // lose a claim or a payer check. Up to 8 retries, starting at 800ms.
        let client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new(8, 800, 100))
            .http(rpc.parse().context("BASE_RPC is not a URL")?);
        let provider = ProviderBuilder::new().wallet(EthereumWallet::from(signer)).connect_client(client).erased();
        Ok(Self { provider, authorizer })
    }

    pub fn authorizer(&self) -> Address {
        self.authorizer
    }

    pub async fn channel(&self, id: B256) -> Result<OnChain> {
        let escrow = IX402BatchSettlement::new(ESCROW, &self.provider);
        let c = escrow.channels(id).call().await.context("escrow.channels")?;
        let w = escrow.pendingWithdrawals(id).call().await.context("escrow.pendingWithdrawals")?;
        Ok(OnChain {
            balance: c.balance,
            total_claimed: c.totalClaimed,
            withdraw_finalize_after: w.finalizeAfter.to::<u64>(),
        })
    }

    /// Would the escrow accept this voucher signature right now?
    ///
    /// Mirrors the escrow: ECDSA against `payerAuthorizer` when it is set, otherwise EIP-1271
    /// against `payer`. `Err` only when the chain could not be asked.
    pub async fn voucher_valid(&self, cfg: &ChannelConfig, digest: B256, sig: &Bytes) -> Result<bool> {
        if cfg.payerAuthorizer != Address::ZERO {
            return Ok(Signature::from_raw(sig)
                .and_then(|s| s.recover_address_from_prehash(&digest))
                .map(|a| a == cfg.payerAuthorizer)
                .unwrap_or(false));
        }
        let payer = IERC1271::new(cfg.payer, &self.provider);
        match payer.isValidSignature(digest, sig.clone()).call().await {
            Ok(magic) => Ok(magic == EIP1271_MAGIC),
            Err(e) if chain_said_no(&e).is_some() => Ok(false),
            Err(e) => Err(anyhow!("could not ask the payer contract: {e}")),
        }
    }

    /// Read a Countersign wallet's verdict on `seller`. `None` if `payer` is not one.
    pub async fn countersign_status(&self, payer: Address, seller: Address) -> Option<CountersignStatus> {
        let wallet = ICountersign::new(payer, &self.provider);
        let r = wallet.revocations(seller).call().await.ok()?;
        let paused = wallet.paused().call().await.ok()?;
        Some(CountersignStatus { revoked: r.revoked, reason: r.reason, revoked_at: r.revokedAt, paused })
    }

    /// Dry-run a claim exactly as it would be sent. `Ok(Err(..))` is the escrow saying no.
    pub async fn simulate_claim(
        &self,
        rows: Vec<IX402BatchSettlement::VoucherClaim>,
    ) -> Result<std::result::Result<(), Rejected>> {
        let escrow = IX402BatchSettlement::new(ESCROW, &self.provider);
        match escrow.claim(rows).from(self.authorizer).call().await {
            Ok(_) => Ok(Ok(())),
            Err(e) => match chain_said_no(&e) {
                Some(msg) => Ok(Err(Rejected(msg))),
                None => Err(anyhow!("could not simulate the claim: {e}")),
            },
        }
    }

    /// Cash vouchers in. Moves accounting only; `settle` moves the tokens.
    pub async fn claim(&self, rows: Vec<IX402BatchSettlement::VoucherClaim>) -> Result<TxHash> {
        let escrow = IX402BatchSettlement::new(ESCROW, &self.provider);
        let receipt = escrow.claim(rows).send().await.context("sending claim")?
            .get_receipt().await.context("waiting for the claim receipt")?;
        if !receipt.status() {
            return Err(anyhow!("claim {} reverted on chain", receipt.transaction_hash));
        }
        Ok(receipt.transaction_hash)
    }

    /// Sweep everything claimed for `receiver` into its wallet. Permissionless.
    pub async fn settle(&self, receiver: Address, token: Address) -> Result<TxHash> {
        let escrow = IX402BatchSettlement::new(ESCROW, &self.provider);
        let receipt = escrow.settle(receiver, token).send().await.context("sending settle")?
            .get_receipt().await.context("waiting for the settle receipt")?;
        if !receipt.status() {
            return Err(anyhow!("settle {} reverted on chain", receipt.transaction_hash));
        }
        Ok(receipt.transaction_hash)
    }
}
