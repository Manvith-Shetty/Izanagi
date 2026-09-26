//! The demo seller.
//!
//!   seller    -- serve `GET /v1/data` at 0.01 USDC a call, and cash vouchers in when due

use anyhow::Result;
use common::intercepta::Intercepta;
use countersign_seller::{chain::Chain, env::SellerEnv, screen::PayerScreen, server};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // this crate's own .env (wherever cargo was invoked from), then ./.env; refuses to
    // start on a file the parser would only half-read -- see common::utils::load_env
    common::utils::load_env(env!("CARGO_MANIFEST_DIR")).map_err(anyhow::Error::msg)?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "seller=info,countersign_seller=info,common=info".into()),
        )
        .init();

    let env = SellerEnv::new().map_err(|e| anyhow::anyhow!(e))?;
    let chain = Chain::new(&env.rpc, &env.receiver_authorizer_key)?;
    let screen = PayerScreen::new(Intercepta::new(env.intercepta_api_key.clone()), env.screen.clone());

    if env.intercepta_api_key.trim().is_empty() {
        tracing::warn!("INTERCEPTA_API_KEY unset: every payer will be refused (failing closed)");
    }
    tracing::info!(
        "receiver {:#x}, claims authorised and paid for by {:#x} (needs ETH for gas)",
        env.receiver,
        chain.authorizer()
    );
    if env.admin_token.is_none() {
        tracing::info!("SELLER_ADMIN_TOKEN unset: POST /admin/claim is disabled, claims run on schedule only");
    }

    let bind = env.bind.clone();
    let app = Arc::new(server::App::new(env, chain, screen));
    tokio::spawn(server::claim_loop(app.clone()));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("x402 seller listening on http://{bind}{}", server::PAID_PATH);
    axum::serve(listener, server::router(app)).await?;
    Ok(())
}
