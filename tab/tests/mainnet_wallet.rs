//! The same path as `fork_wallet.rs`, on Base MAINNET, through Tab's own code, with real money
//! and a real third-party seller. Spends cents; run it only on purpose.
//!
//!   1. deploy the deposit collector (from the person's account)
//!   2. the person's account creates their wallet from the transaction Tab builds, and Tab's own
//!      check accepts it
//!   3. the person adds USDC
//!   4. Tab's `buy` pays hyperextend for a real BTC candle: the countersigner screens it through
//!      Intercepta and countersigns, the agent opens a tab through the wallet's `openTab`, and
//!      the seller serves against the countersigned voucher
//!   5. no allowance is left standing
//!   6. closing the tab revokes the seller on chain
//!   7. the person starts taking the tab back (the seller's delay is a day) and sweeps the rest
//!
//! Needs a countersigner running against mainnet whose oracle key holds a little ETH:
//!
//!   PERSON_KEY=... AGENT_KEY=... CONTROL_TOKEN=... COUNTERSIGNER_URL=http://127.0.0.1:8797 \
//!     cargo test -p tab --test mainnet_wallet -- --ignored --nocapture

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use common::escrow::{ICountersign, IERC20};
use common::{ESCROW, USDC_BASE};
use std::sync::Arc;
use std::time::Duration;
use tab::brain::Brain;
use tab::buyer::{Buyer, Outcome};
use tab::chain::{Chain, OwnerTx};
use tab::feed::Feed;
use tab::store::{now, secret, Account, Store};

const COLLECTOR_BIN: &str = include_str!("../assets/CountersignCollector.bin");
const SELLER_URL: &str = "https://api.hyperextend.xyz/v1/candles/BTC/1m/latest";

fn var(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} unset: see the header of this file"))
}

async fn send(p: &DynProvider, req: TransactionRequest, what: &str) -> alloy::rpc::types::TransactionReceipt {
    let r = p.send_transaction(req).await.unwrap().get_receipt().await.unwrap();
    assert!(r.status(), "{what} reverted: {:#x}", r.transaction_hash);
    println!("  {what}: https://basescan.org/tx/{:#x}", r.transaction_hash);
    r
}

async fn send_owner_tx(p: &DynProvider, tx: &OwnerTx) -> alloy::rpc::types::TransactionReceipt {
    let req = TransactionRequest::default().with_input(tx.data.clone());
    let req = match tx.to {
        Some(to) => req.with_to(to),
        None => req.into_create(),
    };
    send(p, req, &tx.label).await
}

/// The person's account, on a client that retries when a public RPC rate-limits (HTTP 429).
fn person_provider(rpc: &str, person: &PrivateKeySigner) -> DynProvider {
    let client = alloy::rpc::client::ClientBuilder::default()
        .layer(alloy::transports::layers::RetryBackoffLayer::new(8, 800, 100))
        .http(rpc.parse().unwrap());
    ProviderBuilder::new().wallet(EthereumWallet::from(person.clone())).connect_client(client).erased()
}

/// Public RPCs balance reads across nodes that can lag a block behind: ask again for a minute.
async fn revoked_soon(chain: &Chain, wallet: Address, seller: Address) -> bool {
    for _ in 0..30 {
        if chain.revoked(wallet, seller).await.unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

/// Wait for `who` to hold `want` USDC, as a lagging RPC node catches up.
async fn usdc_soon(p: &DynProvider, who: Address, want: u128) -> bool {
    for _ in 0..30 {
        if usdc(p, who).await == want {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

async fn usdc(p: &DynProvider, who: Address) -> u128 {
    IERC20::new(USDC_BASE, p).balanceOf(who).call().await.unwrap().to::<u128>()
}

/// Steps 6 and 7 alone, for a wallet an earlier run created and paid from (set `WALLET`):
/// close its hyperextend tab on chain, then take the money back out.
#[tokio::test]
#[ignore = "spends real money on Base mainnet"]
async fn close_and_empty_an_existing_mainnet_wallet() {
    let rpc = std::env::var("MAINNET_RPC").unwrap_or_else(|_| "https://mainnet.base.org".into());
    let person: PrivateKeySigner = var("PERSON_KEY").trim().parse().unwrap();
    let wallet: Address = var("WALLET").parse().unwrap();
    let brain = Brain::new(&var("COUNTERSIGNER_URL"), &var("CONTROL_TOKEN"));
    let p = person_provider(&rpc, &person);
    let collector = ICountersign::new(wallet, &p).collector().call().await.unwrap();
    let chain = Chain::new(&rpc, &var("AGENT_KEY"), collector).unwrap();

    // hyperextend's live terms: its payTo, its claim authorizer, a one-day withdraw delay
    let seller: Address = "0x548fC289526ab2F0391D562a723cdA64Bbf1abc4".parse().unwrap();
    let r_auth: Address = "0x3721824a31197dcDD2984cF43b92B6cc8A87c0Fb".parse().unwrap();
    let cfg = common::ChannelConfig::countersign(wallet, seller, r_auth, USDC_BASE, 86_400, alloy::primitives::B256::ZERO);
    let id = common::channel_id(&cfg, 8453);
    let on = chain.tab(id).await.unwrap();
    assert!(on.balance > 0, "no tab with hyperextend on {wallet:#x}");

    // 6. close the tab: the seller is revoked on chain
    if !chain.revoked(wallet, seller).await.unwrap() {
        let closed = brain.revoke(wallet, seller, Some("mainnet check")).await.unwrap();
        println!("  closed: {closed}");
    }
    assert!(revoked_soon(&chain, wallet, seller).await, "the seller is revoked on chain");
    println!("  hyperextend is revoked on {wallet:#x}");

    // 7. money out: start taking the tab back, sweep the rest
    let before = usdc(&p, person.address()).await;
    let idle = usdc(&p, wallet).await;
    let latest = p.get_block_by_number(alloy::eips::BlockNumberOrTag::Latest).await.unwrap().unwrap().header.timestamp;
    for tx in &OwnerTx::withdraw(wallet, person.address(), idle, &[(cfg, on, "hyperextend".into())], latest) {
        send_owner_tx(&p, tx).await;
    }
    assert!(usdc_soon(&p, person.address(), before + idle).await, "the wallet's balance is back");
    let after = chain.tab(id).await.unwrap();
    println!(
        "  left in the tab: {} (unclaimed by the seller); finish taking it back after {} (unix)",
        after.balance.saturating_sub(after.total_claimed),
        after.withdraw_started_at + 86_400
    );
}

#[tokio::test]
#[ignore = "spends real money on Base mainnet"]
async fn a_person_creates_a_wallet_and_pays_a_real_seller_on_mainnet() {
    let rpc = std::env::var("MAINNET_RPC").unwrap_or_else(|_| "https://mainnet.base.org".into());
    let person: PrivateKeySigner = var("PERSON_KEY").trim().parse().unwrap();
    let agent_key = var("AGENT_KEY");
    let agent: PrivateKeySigner = agent_key.trim().parse().unwrap();
    let cs_url = var("COUNTERSIGNER_URL");
    let brain = Brain::new(&cs_url, &var("CONTROL_TOKEN"));
    let oracle: Address = brain.health().await.unwrap()["oracle"].as_str().unwrap().parse().unwrap();

    let p = person_provider(&rpc, &person);
    assert_eq!(p.get_chain_id().await.unwrap(), 8453, "not Base mainnet");
    println!("person {:#x}, agent {:#x}, oracle {oracle:#x}", person.address(), agent.address());

    // gas for the agent, which opens the tab
    if p.get_balance(agent.address()).await.unwrap() < U256::from(50_000_000_000_000u64) {
        send(&p, TransactionRequest::default().with_to(agent.address()).with_value(U256::from(100_000_000_000_000u64)), "gas for the agent").await;
    }

    // 1. the collector
    let init: Bytes = [alloy::hex::decode(COLLECTOR_BIN.trim()).unwrap(), ESCROW.abi_encode()].concat().into();
    let collector = send(&p, TransactionRequest::default().with_deploy_code(init), "deploy the collector")
        .await
        .contract_address
        .unwrap();
    let chain = Arc::new(Chain::new(&rpc, &agent_key, collector).unwrap());

    // 2. the person's own account creates their wallet, and Tab accepts exactly that
    let r = send_owner_tx(&p, &OwnerTx::create_wallet(chain.wallet_initcode(person.address(), oracle).unwrap())).await;
    let wallet = chain.created_wallet(r.transaction_hash, person.address(), oracle, Duration::from_secs(60)).await.unwrap();
    let w = ICountersign::new(wallet, &p);
    assert_eq!(w.owner().call().await.unwrap(), person.address());
    assert_eq!(w.collector().call().await.unwrap(), collector);
    println!("  wallet: https://basescan.org/address/{wallet:#x}");

    // 3. money in
    let tx = send_owner_tx(&p, &OwnerTx::deposit(wallet, 20_000)).await.transaction_hash;
    let d = chain.deposit_into(wallet, tx, Duration::from_secs(60)).await.unwrap();
    assert_eq!((d.from, d.amount), (person.address(), 20_000));

    // 4. Tab's own purchase path, against a real seller
    let dir = std::env::temp_dir().join(format!("tab-mainnet-{}", now()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(Store::load(dir.join("tab.json")).unwrap());
    let feed = Arc::new(Feed::default());
    let buyer = Buyer::new(&cs_url, &agent_key, brain.clone(), chain.clone(), store.clone(), feed, 8453, 5_000, 100_000).unwrap();
    let account = Account {
        id: "acct_mainnet_check".into(),
        human: "mainnet-check".into(),
        wallet,
        created_at: now(),
        mcp_token: secret("tok"),
        deploy_tx: Some(format!("{:#x}", r.transaction_hash)),
    };
    store.put_account(account.clone()).await.unwrap();
    let paid = match buyer.buy(&account, SELLER_URL, None).await {
        Outcome::Paid(p) => p,
        other => panic!("not paid: {other:?}"),
    };
    println!("  paid {} to {:#x} ({}), got: {}", paid.price, paid.seller, paid.verdict, paid.data.to_string().chars().take(160).collect::<String>());
    if let Some(o) = &paid.opened {
        println!("  tab opened: https://basescan.org/tx/{}", o.tx);
    }
    assert_eq!(paid.status, 200);

    // 5. nothing left for anyone to pull
    let allowance = IERC20::new(USDC_BASE, &p).allowance(wallet, collector).call().await.unwrap();
    assert!(allowance.is_zero(), "no allowance left standing");

    // 6. close the tab: the seller is revoked on chain
    let closed = brain.revoke(wallet, paid.seller, Some("mainnet check")).await.unwrap();
    println!("  closed: {closed}");
    assert!(revoked_soon(&chain, wallet, paid.seller).await, "the seller is revoked on chain");

    // 7. money out: start taking the tab back, sweep the rest
    let t = store.tab(paid.channel_id).await.unwrap();
    let on = chain.tab(paid.channel_id).await.unwrap();
    let before = usdc(&p, person.address()).await;
    let idle = usdc(&p, wallet).await;
    let latest = p.get_block_by_number(alloy::eips::BlockNumberOrTag::Latest).await.unwrap().unwrap().header.timestamp;
    let plan = OwnerTx::withdraw(wallet, person.address(), idle, &[(t.config(wallet), on, "hyperextend".into())], latest);
    for tx in &plan {
        send_owner_tx(&p, tx).await;
    }
    assert!(usdc_soon(&p, person.address(), before + idle).await, "the wallet's balance is back");
    let after = chain.tab(paid.channel_id).await.unwrap();
    println!(
        "  left in the tab: {} (unclaimed by the seller); finish taking it back after {} (unix)",
        after.balance.saturating_sub(after.total_claimed),
        after.withdraw_started_at + 86_400
    );
}
