//! A person's whole path against Coinbase's real escrow on a fork of Base: their own account
//! creates their wallet (as MetaMask would, from the transaction Tab builds), Tab checks it and
//! refuses anything else, the agent opens a tab with no allowance left behind, they add USDC,
//! and they take every cent back out. Tab holds no key that owns or funds their wallet.
//!
//!   anvil --fork-url https://mainnet.base.org --port 8646
//!   TAB_FORK_RPC=http://127.0.0.1:8646 cargo test -p tab --test fork_wallet -- --ignored --nocapture
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
use std::time::Duration;
use tab::chain::{Chain, OwnerTx};

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

/// Send a transaction Tab built, the way a wallet app would: `to` absent means "create".
async fn send(p: &DynProvider, tx: &OwnerTx) -> alloy::rpc::types::TransactionReceipt {
    let mut req = TransactionRequest::default().with_input(tx.data.clone());
    req = match tx.to {
        Some(to) => req.with_to(to),
        None => req.into_create(),
    };
    let r = p.send_transaction(req).await.unwrap().get_receipt().await.unwrap();
    assert!(r.status(), "{} reverted", tx.label);
    r
}

async fn usdc(p: &DynProvider, who: Address) -> u128 {
    IERC20::new(USDC_BASE, p).balanceOf(who).call().await.unwrap().to::<u128>()
}

async fn chain_now(p: &DynProvider) -> u64 {
    p.get_block_by_number(alloy::eips::BlockNumberOrTag::Latest).await.unwrap().unwrap().header.timestamp
}

#[tokio::test]
#[ignore = "needs a Base fork"]
async fn a_person_creates_funds_uses_and_empties_their_own_wallet() {
    let rpc = std::env::var("TAB_FORK_RPC").expect("TAB_FORK_RPC unset: see the header of this file");
    let (setup, agent, person, stranger, oracle) = (key("setup"), key("agent"), key("person"), key("stranger"), key("oracle"));
    let read = ProviderBuilder::new().connect(&rpc).await.unwrap().erased();
    fund(&read, setup.address(), 0).await;
    fund(&read, agent.address(), 0).await;
    fund(&read, person.address(), 5_000_000).await;
    fund(&read, stranger.address(), 0).await;

    // the shared collector, deployed once at setup from the same bytecode Tab ships
    let init: Bytes = [alloy::hex::decode(COLLECTOR_BIN.trim()).unwrap(), ESCROW.abi_encode()].concat().into();
    let s = signer(&rpc, &setup).await;
    let collector = s
        .send_transaction(TransactionRequest::default().with_deploy_code(init))
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap()
        .contract_address
        .expect("collector deployed");

    let chain = Chain::new(&rpc, &hex_key(&agent), collector).unwrap();
    let p = signer(&rpc, &person).await;

    // 1. the person's own account creates their wallet from the transaction Tab builds
    let create = OwnerTx::create_wallet(chain.wallet_initcode(person.address(), oracle.address()).unwrap());
    let r = send(&p, &create).await;
    let wallet = chain
        .created_wallet(r.transaction_hash, person.address(), oracle.address(), Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(Some(wallet), r.contract_address);
    let w = ICountersign::new(wallet, &read);
    assert_eq!(w.owner().call().await.unwrap(), person.address(), "theirs from the first block");
    assert_eq!(w.agent().call().await.unwrap(), chain.agent_address);
    assert_eq!(w.riskOracle().call().await.unwrap(), oracle.address());
    assert_eq!(w.collector().call().await.unwrap(), collector);

    // someone else's creation cannot be claimed as theirs...
    assert!(chain
        .created_wallet(r.transaction_hash, stranger.address(), oracle.address(), Duration::from_secs(5))
        .await
        .is_err());
    // ...and a wallet wired to other keys is not a Tab wallet
    let rogue = OwnerTx::create_wallet(
        Chain::new(&rpc, &hex_key(&stranger), collector).unwrap().wallet_initcode(person.address(), oracle.address()).unwrap(),
    );
    let r2 = send(&p, &rogue).await;
    assert!(chain
        .created_wallet(r2.transaction_hash, person.address(), oracle.address(), Duration::from_secs(5))
        .await
        .is_err());

    // 2. they add money from their own account
    let tx = send(&p, &OwnerTx::deposit(wallet, 1_000_000)).await.transaction_hash;
    let d = chain.deposit_into(wallet, tx, Duration::from_secs(20)).await.unwrap();
    assert_eq!((d.from, d.amount), (person.address(), 1_000_000));

    // 3. the agent opens tabs from the wallet's own funds, leaving no allowance behind
    let seller = Address::repeat_byte(0x5e);
    let cfg = ChannelConfig::countersign(wallet, seller, seller, USDC_BASE, 86_400, B256::repeat_byte(1));
    let cfg2 = ChannelConfig::countersign(wallet, seller, seller, USDC_BASE, 86_400, B256::repeat_byte(2));
    chain.open_tab(&cfg, 100_000).await.unwrap();
    chain.open_tab(&cfg2, 50_000).await.unwrap();
    let (id, id2) = (channel_id(&cfg, 8453), channel_id(&cfg2, 8453));
    assert_eq!(chain.tab(id).await.unwrap().balance, 100_000, "the escrow holds the tab");
    let allowance = IERC20::new(USDC_BASE, &read).allowance(wallet, collector).call().await.unwrap();
    assert!(allowance.is_zero(), "no allowance left standing");
    assert_eq!(usdc(&read, wallet).await, 850_000);

    // 4. they take it all out: start both tabs' withdrawals and sweep the wallet...
    let before = usdc(&read, person.address()).await;
    let tabs = |a, b| vec![(cfg.clone(), a, "one".to_string()), (cfg2.clone(), b, "two".to_string())];
    let idle = usdc(&read, wallet).await;
    let plan = OwnerTx::withdraw(wallet, person.address(), idle, &tabs(chain.tab(id).await.unwrap(), chain.tab(id2).await.unwrap()), chain_now(&read).await);
    assert_eq!(plan.len(), 3, "{plan:#?}");
    for tx in &plan {
        send(&p, tx).await;
    }
    assert_eq!(usdc(&read, person.address()).await, before + idle);

    // ...and after the seller's wait, finish them and sweep what came back
    let _: U256 = read.raw_request("evm_increaseTime".into(), (86_401u64,)).await.unwrap();
    let _: String = read.raw_request("evm_mine".into(), ()).await.unwrap();
    let plan = OwnerTx::withdraw(wallet, person.address(), 0, &tabs(chain.tab(id).await.unwrap(), chain.tab(id2).await.unwrap()), chain_now(&read).await);
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
