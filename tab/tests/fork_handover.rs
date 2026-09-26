//! The whole "bring your own money" path against Coinbase's real escrow on a fork of Base:
//! a trial wallet opens a tab with no allowance left behind, a person adds USDC from their own
//! account, Tab hands the wallet to that account, and the person takes everything back out.
//!
//!   anvil --fork-url https://mainnet.base.org --port 8645
//!   TAB_FORK_RPC=http://127.0.0.1:8645 cargo test -p tab --test fork_handover -- --ignored --nocapture
//!
//! Every key is derived from a fixed label and funded on the fork, so nothing else is needed.

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use common::escrow::{ICountersign, IERC20};
use common::{channel_id, ChannelConfig, ESCROW, USDC_BASE};
use tab::chain::{Chain, OwnerTx};
use std::time::Duration;

const COLLECTOR_BIN: &str = include_str!("../assets/CountersignCollector.bin");

fn key(label: &str) -> PrivateKeySigner {
    PrivateKeySigner::from_bytes(&keccak256(format!("tab-fork-{label}"))).unwrap()
}

fn hex_key(k: &PrivateKeySigner) -> String {
    format!("0x{}", alloy::hex::encode(k.to_bytes()))
}

async fn signer(rpc: &str, k: &PrivateKeySigner) -> DynProvider {
    ProviderBuilder::new().wallet(EthereumWallet::from(k.clone())).connect(rpc).await.unwrap().erased()
}

/// Give `who` gas, and `usdc` of real Base USDC (FiatTokenV2_2 keeps balances at slot 9).
async fn fund(p: &DynProvider, who: Address, usdc: u128) {
    let _: () = p.raw_request("anvil_setBalance".into(), (who, U256::from(10u128.pow(18)))).await.unwrap();
    let _: () = p.raw_request("anvil_setCode".into(), (who, Bytes::new())).await.unwrap();
    let slot = keccak256((who, U256::from(9)).abi_encode());
    let _: bool = p
        .raw_request("anvil_setStorageAt".into(), (USDC_BASE, slot, B256::from(U256::from(usdc))))
        .await
        .unwrap();
}

async fn send(p: &DynProvider, tx: &OwnerTx) -> B256 {
    let req = TransactionRequest::default().with_to(tx.to).with_input(tx.data.clone());
    let r = p.send_transaction(req).await.unwrap().get_receipt().await.unwrap();
    assert!(r.status(), "{} reverted", tx.label);
    r.transaction_hash
}

async fn usdc(p: &DynProvider, who: Address) -> u128 {
    IERC20::new(USDC_BASE, p).balanceOf(who).call().await.unwrap().to::<u128>()
}

async fn chain_now(p: &DynProvider) -> u64 {
    p.get_block_by_number(alloy::eips::BlockNumberOrTag::Latest).await.unwrap().unwrap().header.timestamp
}

#[tokio::test]
#[ignore = "needs a Base fork"]
async fn a_trial_wallet_becomes_the_persons_and_they_take_it_all_out() {
    let rpc = std::env::var("TAB_FORK_RPC").expect("TAB_FORK_RPC unset: see the header of this file");
    let (treasury, agent, person, oracle) = (key("treasury"), key("agent"), key("person"), key("oracle"));
    let read = ProviderBuilder::new().connect(&rpc).await.unwrap().erased();
    fund(&read, treasury.address(), 1_000_000).await;
    fund(&read, agent.address(), 0).await;
    fund(&read, person.address(), 5_000_000).await;

    // the shared collector, deployed from the same bytecode Tab ships
    let t = signer(&rpc, &treasury).await;
    let init: Bytes = [alloy::hex::decode(COLLECTOR_BIN.trim()).unwrap(), ESCROW.abi_encode()].concat().into();
    let r = t.send_transaction(TransactionRequest::default().with_deploy_code(init)).await.unwrap().get_receipt().await.unwrap();
    let collector = r.contract_address.expect("collector deployed");

    let chain = Chain::new(&rpc, &hex_key(&agent), &hex_key(&treasury), collector).unwrap();

    // 1. a free trial: Tab's treasury owns it, and the agent has a collector but no allowance
    let (wallet, _) = chain.deploy_wallet(oracle.address()).await.unwrap();
    chain.fund_wallet(wallet, 250_000).await.unwrap();
    let w = chain.wallet(wallet).await.unwrap();
    assert_eq!(w.owner, chain.treasury_address);
    assert_eq!(ICountersign::new(wallet, &read).collector().call().await.unwrap(), collector);
    let allowance = || async { IERC20::new(USDC_BASE, &read).allowance(wallet, collector).call().await.unwrap() };
    assert!(allowance().await.is_zero(), "no allowance standing after the trial is funded");

    // 2. the agent opens a tab from the wallet's own funds
    let seller = Address::repeat_byte(0x5e);
    let cfg = ChannelConfig::countersign(wallet, seller, seller, USDC_BASE, 86_400, B256::repeat_byte(1));
    chain.open_tab(&cfg, 100_000).await.unwrap();
    let id = channel_id(&cfg, 8453);
    assert_eq!(chain.tab(id).await.unwrap().balance, 100_000, "the escrow holds the tab");
    assert!(allowance().await.is_zero(), "no allowance left behind by the tab");
    assert_eq!(usdc(&read, wallet).await, 150_000);

    // 3. the person adds money from their own account, exactly the transaction Tab builds
    let p = signer(&rpc, &person).await;
    let tx = send(&p, &OwnerTx::deposit(wallet, 1_000_000)).await;
    let d = chain.deposit_into(wallet, tx, Duration::from_secs(20)).await.unwrap();
    assert_eq!((d.from, d.amount), (person.address(), 1_000_000));

    // a transaction that sent nothing to this wallet proves nothing
    let elsewhere = send(&p, &OwnerTx::deposit(Address::repeat_byte(0x42), 10_000)).await;
    assert!(chain.deposit_into(wallet, elsewhere, Duration::from_secs(5)).await.is_err());

    // 4. the wallet becomes theirs, once
    chain.hand_over(wallet, d.from).await.unwrap();
    assert_eq!(chain.wallet(wallet).await.unwrap().owner, person.address());
    assert!(chain.hand_over(wallet, chain.treasury_address).await.is_err(), "Tab has no say any more");

    // the agent still opens tabs for them
    let cfg2 = ChannelConfig::countersign(wallet, seller, seller, USDC_BASE, 86_400, B256::repeat_byte(2));
    chain.open_tab(&cfg2, 50_000).await.unwrap();

    // 5. they take it all out: start both tabs' withdrawals and sweep the wallet...
    let before = usdc(&read, person.address()).await;
    let tabs = |a, b| vec![(cfg.clone(), a, "one".to_string()), (cfg2.clone(), b, "two".to_string())];
    let idle = usdc(&read, wallet).await;
    let plan = OwnerTx::withdraw(wallet, person.address(), idle, &tabs(chain.tab(id).await.unwrap(), chain.tab(channel_id(&cfg2, 8453)).await.unwrap()), chain_now(&read).await);
    assert_eq!(plan.len(), 3, "{plan:#?}");
    for tx in &plan {
        send(&p, tx).await;
    }
    assert_eq!(usdc(&read, person.address()).await, before + idle);

    // ...and after the seller's wait, finish them and sweep what came back
    let _: U256 = read.raw_request("evm_increaseTime".into(), (86_401u64,)).await.unwrap();
    let _: String = read.raw_request("evm_mine".into(), ()).await.unwrap();
    let plan = OwnerTx::withdraw(wallet, person.address(), 0, &tabs(chain.tab(id).await.unwrap(), chain.tab(channel_id(&cfg2, 8453)).await.unwrap()), chain_now(&read).await);
    assert_eq!(plan.len(), 2, "{plan:#?}");
    for tx in &plan {
        send(&p, tx).await;
    }
    let back = usdc(&read, wallet).await;
    assert_eq!(back, 150_000, "both tabs came back to the wallet");
    send(&p, &OwnerTx::withdraw(wallet, person.address(), back, &[], 0)[0]).await;
    assert_eq!(usdc(&read, person.address()).await, before + idle + back, "every cent is back with its owner");
    assert_eq!(usdc(&read, wallet).await, 0);
}
