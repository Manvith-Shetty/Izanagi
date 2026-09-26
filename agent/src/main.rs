//! Demo agent.
//!
//!   agent scan            -- profile the live x402 Bazaar (the opening slide)
//!   agent pay <seller>    -- ask the countersigner for one voucher, and show it
//!   agent fetch <url> [n] -- buy n requests from a live x402 batch-settlement seller

use alloy::primitives::{Address, B256};
use alloy::providers::ProviderBuilder;
use anyhow::Result;
use common::escrow::IX402BatchSettlement;
use common::x402::parse_amount;
use common::{ChannelConfig, USDC_BASE};
use countersign_agent::{
    bazaar,
    client::{CountersignClient, SignedVoucher},
    env::AgentEnv,
    x402,
};

fn usdc(v: u128) -> String {
    format!("{}.{:06}", v / 1_000_000, v % 1_000_000)
}

async fn scan() -> Result<()> {
    println!("querying Coinbase's live x402 Bazaar (no auth required)...\n");
    let listings = bazaar::discover(1000, None).await?;
    let s = bazaar::stats(&listings);
    println!("  listings                    {}", s.listings);
    println!("  distinct seller addresses   {}", s.sellers);
    println!("  networks                    {}", s.networks);
    println!("  median price                {} USDC", usdc(s.median_price));
    println!("  listings >= 1,000 USDC/call {}", s.over_1k);
    println!("  most expensive              {} USDC", usdc(s.max_price));
    println!("    -> {}", s.max_resource);
    println!("\nAn agent can pull this list with one unauthenticated HTTP call and start");
    println!("paying any of these {} strangers. Nothing checks who they are.", s.sellers);
    Ok(())
}

/// Ask the countersigner for one voucher, walking a human through approval when the verdict
/// is `ask`. `Ok(None)` means no voucher exists: refused, denied, expired or cancelled.
async fn authorize_interactive(
    c: &CountersignClient,
    channel: &ChannelConfig,
    chain_id: u64,
    ceiling: u128,
) -> Result<Option<SignedVoucher>> {
    match c.authorize(channel, chain_id, ceiling, None).await {
        Ok((res, Some(v))) => {
            println!("  verdict     {}", res.action());
            println!("  reason      {}", res.reason);
            println!("  toxicScore  {}", res.toxic_score);
            Ok(Some(v))
        }
        Ok((res, None)) => {
            println!("  verdict     {}", res.action());
            println!("  reason      {}", res.reason);
            let Some(d) = res.approval else {
                println!("\n  A human is required, but World ID is not configured on the");
                println!("  countersigner - so this is an effective refusal, not a silent pass.");
                return Ok(None);
            };

            println!("\n  ── a human has to approve this one ─────────────────────────");
            println!("     approving: {} USDC to {}", usdc(d.ceiling), d.seller);
            println!("     open : {}", d.verification_uri);
            println!("     code : {}", d.user_code);
            if let Some(c) = &d.verification_uri_complete {
                println!("     or   : {c}");
            }
            println!("  ────────────────────────────────────────────────────────────");
            println!("  No signature exists until they approve. Waiting...\n");

            if let Err(e) = c
                .await_approval(&d.approval_id, d.interval, d.expires_in, |waited| {
                    println!("  ... waiting on the human ({waited}s)");
                })
                .await
            {
                // the denied / expired / cancelled path
                println!("\n  NOT APPROVED: {e}");
                println!("  No countersignature was produced, so no voucher exists.");
                println!("  A claim submitted anyway reverts on-chain.");
                return Ok(None);
            }

            println!("\n  approved by a human");
            println!("  (the subject stays in the countersigner - we only get a yes)");

            // redeem the approval, which is bound to THIS payment and usable once
            match c.authorize(channel, chain_id, ceiling, Some(&d.approval_id)).await {
                Ok((res2, Some(v))) => {
                    println!("  verdict     {}", res2.action());
                    Ok(Some(v))
                }
                Ok((res2, None)) => {
                    println!("  still not authorised: {}", res2.reason);
                    Ok(None)
                }
                Err(e) => {
                    println!("  REFUSED after approval: {e}");
                    Ok(None)
                }
            }
        }
        Err(e) => {
            println!("  REFUSED: {e}");
            Ok(None)
        }
    }
}

async fn pay(seller: &str, ceiling: u128) -> Result<()> {
    let cfg = AgentEnv::new().map_err(|e| anyhow::anyhow!(e))?;
    let wallet: Address = cfg.countersign_wallet.parse()?;
    let r_auth: Address = cfg
        .receiver_authorizer
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("RECEIVER_AUTHORIZER is required for `agent pay`"))?
        .parse()?;

    let c = CountersignClient::new(cfg.countersigner_url.clone(), &cfg.agent_private_key)?;
    println!("agent  {:#x}", c.address());
    println!("seller {seller}");
    println!("asking the countersigner to authorise {} USDC...\n", usdc(ceiling));

    let channel = ChannelConfig::countersign(wallet, seller.parse()?, r_auth, USDC_BASE, 900, B256::ZERO);

    if let Some(v) = authorize_interactive(&c, &channel, cfg.chain_id, ceiling).await? {
        println!("  ceiling     {} USDC", usdc(v.ceiling));
        println!("  digest      {:#x}", v.digest);
        println!("  blob        {} bytes (agent sig + oracle sig + seller + expiry)", v.signature_blob.len());
        println!("\nThis voucher is claimable -- for now. If the seller's score moves");
        println!("before they cash it in, it stops being claimable.");
    }
    Ok(())
}

/// Buy `count` requests from a live x402 `batch-settlement` seller.
async fn fetch(url: &str, count: u32) -> Result<()> {
    let cfg = AgentEnv::new().map_err(|e| anyhow::anyhow!(e))?;
    let wallet: Address = cfg.countersign_wallet.parse()?;
    let c = CountersignClient::new(cfg.countersigner_url.clone(), &cfg.agent_private_key)?;
    let http = reqwest::Client::new();

    // 1. the seller's terms
    let res = http.get(url).send().await?;
    if res.status() != reqwest::StatusCode::PAYMENT_REQUIRED {
        anyhow::bail!("expected 402 Payment Required from {url}, got {}", res.status());
    }
    let header = res
        .headers()
        .get(common::x402::HEADER_REQUIRED)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("402 without a PAYMENT-REQUIRED header"))?;
    let offer = x402::terms_from_402(header, cfg.chain_id)?;
    let terms = offer.terms.clone();
    let price = parse_amount(&terms.amount)?;
    let channel = x402::channel_for(&terms, wallet, B256::ZERO);
    let id = common::channel_id(&channel, cfg.chain_id);

    // 2. cold start from chain (spec): whatever is already claimed is our baseline
    let rpc = ProviderBuilder::new().connect_http(cfg.base_rpc.parse()?);
    let on = IX402BatchSettlement::new(common::ESCROW, &rpc).channels(id).call().await?;
    let mut charged = on.totalClaimed;

    println!("seller    {}  ({} USDC a call)", terms.pay_to, usdc(price));
    println!("channel   {id:#x}");
    println!("escrowed  {} USDC, {} already claimed\n", usdc(on.balance), usdc(on.totalClaimed));

    // 3. one countersigned cumulative voucher per request
    for n in 1..=count {
        let ceiling = charged + price;
        println!("request {n}: authorising a cumulative {} USDC", usdc(ceiling));
        let Some(v) = authorize_interactive(&c, &channel, cfg.chain_id, ceiling).await? else {
            println!("\nstopped: no voucher, so nothing was sent to the seller.");
            return Ok(());
        };
        if v.ceiling < ceiling {
            println!("  capped at   {} USDC", usdc(v.ceiling));
            println!("\nstopped: the risk verdict caps what this seller can ever claim below");
            println!("what the next request costs. The payment is smaller, not refused outright.");
            return Ok(());
        }

        let res = http
            .get(url)
            .header(common::x402::HEADER_SIGNATURE, x402::payment_header(&offer, &channel, &v)?)
            .send()
            .await?;
        let status = res.status();
        let receipt = res.headers().get(common::x402::HEADER_RESPONSE).and_then(|h| h.to_str().ok()).map(String::from);
        let body: serde_json::Value = res.json().await.unwrap_or_default();

        if status.is_success() {
            charged += x402::charged_from_response(receipt.as_deref(), price)?;
            println!("  seller      200 {body}");
            println!("  charged     {} USDC so far, all of it still cancellable until claimed\n", usdc(charged));
        } else {
            println!("  seller      {status}: {}", body.get("error").unwrap_or(&body));
            if let Some(m) = body.get("message").or_else(|| body.get("reason")) {
                println!("              {}", m.as_str().unwrap_or_default());
            }
            return Ok(());
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // this crate's own .env (wherever cargo was invoked from), then ./.env; refuses to
    // start on a file the parser would only half-read -- see common::utils::load_env
    common::utils::load_env(env!("CARGO_MANIFEST_DIR")).map_err(anyhow::Error::msg)?;
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("scan") => scan().await,
        Some("pay") => {
            let seller = args.get(2).map(|s| s.as_str())
                .ok_or_else(|| anyhow::anyhow!("usage: agent pay <seller> [ceiling]"))?;
            let ceiling = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5_000_000);
            pay(seller, ceiling).await
        }
        Some("fetch") => {
            let url = args.get(2).map(|s| s.as_str())
                .ok_or_else(|| anyhow::anyhow!("usage: agent fetch <url> [count]"))?;
            let count = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
            fetch(url, count).await
        }
        _ => {
            println!("usage:\n  agent scan\n  agent pay <seller-address> [ceiling-atomic]\n  agent fetch <url> [count]");
            Ok(())
        }
    }
}
