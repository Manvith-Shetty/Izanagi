//! Tab's own chain code against a fork of Base: deploy a person's wallet, fund their trial,
//! and check the result from the chain's side.
//!
//!   anvil --fork-url https://mainnet.base.org --port 8645
//!   RPC=http://127.0.0.1:8645 scripts/fork-setup.sh --write-env   (or ENV_ROOT=... for scratch)
//!   TAB_FORK_RPC=... AGENT_PRIVATE_KEY=... TAB_TREASURY_KEY=... COUNTERSIGN_COLLECTOR=... ORACLE=... \
//!     cargo test -p tab --test fork_chain -- --ignored --nocapture

use alloy::primitives::Address;
use tab::chain::Chain;

fn var(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} unset: see the header of this file"))
}

#[tokio::test]
#[ignore = "needs a Base fork"]
async fn deploy_and_fund_a_persons_wallet() {
    let collector: Address = var("COUNTERSIGN_COLLECTOR").parse().unwrap();
    let oracle: Address = var("ORACLE").parse().unwrap();
    let chain = Chain::new(&var("TAB_FORK_RPC"), &var("AGENT_PRIVATE_KEY"), &var("TAB_TREASURY_KEY"), collector).unwrap();

    let (wallet, tx) = chain.deploy_wallet(oracle).await.unwrap();
    println!("deployed {wallet:#x} in {tx:#x}");
    let (fund, allow) = chain.fund_wallet(wallet, 250_000).await.unwrap();
    println!("funded in {fund:#x}, allowance in {allow:#x}");

    let w = chain.wallet(wallet).await.unwrap();
    assert_eq!(w.owner, chain.treasury_address, "the treasury owns trial wallets");
    assert_eq!(w.risk_oracle, oracle, "the countersigner guards it");
    assert_eq!(w.usdc, 250_000, "the trial landed");
    assert_eq!(w.allowance, 250_000, "the collector may open tabs with it");
    assert!(!w.paused);
    println!("WALLET={wallet:#x}");
}
