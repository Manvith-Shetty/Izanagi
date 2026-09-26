//! Emits a signing fixture produced by the REAL Rust countersigner, so the Solidity
//! fork test can prove that Coinbase's deployed escrow accepts our signatures.
//!
//!   cargo run -p countersigner --bin gen_fixture > contracts/fixture.json

use alloy::primitives::{address, b256, Address};
use alloy::signers::{local::PrivateKeySigner, SignerSync};
use common::{channel_id, voucher_digest, ChannelConfig, USDC_BASE};

const ORACLE_KEY: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const AGENT_KEY: &str = "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba";

fn hexs(b: &[u8]) -> String {
    let mut s = String::from("0x");
    for x in b { s.push_str(&format!("{x:02x}")); }
    s
}

fn main() -> anyhow::Result<()> {
    let oracle: PrivateKeySigner = ORACLE_KEY.parse()?;
    let agent: PrivateKeySigner = AGENT_KEY.parse()?;

    // the Countersign wallet address is passed in: it is part of channel identity,
    // so both sides must agree on it before anything is signed.
    let wallet: Address = std::env::args()
        .nth(1)
        .expect("usage: gen_fixture <countersign-wallet-address>")
        .parse()?;
    let seller = address!("00000000000000000000000000000000005E11E5");
    let r_auth = address!("6E9972213BF459853FA33E28Ab7219e9157C8d02"); // vm.addr(0xBEEF)

    let chain_id: u64 = 8453;
    let ceiling: u128 = 5_000_000;
    let expiry: u64 = 4_000_000_000;

    let cfg = ChannelConfig::countersign(wallet, seller, r_auth, USDC_BASE, 900, b256!("0000000000000000000000000000000000000000000000000000000000000001"));
    let cid = channel_id(&cfg, chain_id);
    let digest = voucher_digest(cid, ceiling, chain_id);
    let att = common::attestation_hash(digest, seller, expiry);

    let agent_sig = agent.sign_hash_sync(&digest)?;
    let oracle_sig = oracle.sign_hash_sync(&att)?;

    println!("{}", serde_json::json!({
        "wallet": format!("{wallet:#x}"),
        "seller": format!("{seller:#x}"),
        "receiverAuthorizer": format!("{r_auth:#x}"),
        "agent": format!("{:#x}", agent.address()),
        "oracle": format!("{:#x}", oracle.address()),
        "chainId": chain_id,
        "ceiling": ceiling,
        "expiry": expiry,
        "channelId": format!("{cid:#x}"),
        "digest": format!("{digest:#x}"),
        "agentSig": hexs(&agent_sig.as_bytes()),
        "oracleSig": hexs(&oracle_sig.as_bytes()),
    }));
    Ok(())
}
