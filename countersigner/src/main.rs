//! Countersign -- the countersigner.
//!
//! Holds the Intercepta key, the World ID client secret, the budget ledger and the
//! countersigning key. The agent-side library holds none of these: it asks, this decides.
//!
//! What this service can do:  decline, cap, demand a human, revoke a seller mid-session.
//! What it cannot do:         move money, redirect money, or trap money.

use anyhow::Result;
use countersigner_lib::{api, app::App, env};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // this crate's own .env (wherever cargo was invoked from), then ./.env; refuses to
    // start on a file the parser would only half-read -- see common::utils::load_env
    common::utils::load_env(env!("CARGO_MANIFEST_DIR")).map_err(anyhow::Error::msg)?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "countersigner=info,countersigner_lib=info,common=info".into()),
        )
        .init();

    let cfg = env::CountersignerEnv::new().map_err(|e| anyhow::anyhow!(e))?;
    let app = Arc::new(App::from_env(&cfg)?);

    // the loop that keeps screening open sessions
    tokio::spawn(app.clone().watch());

    tracing::info!(
        oracle = %format!("{:#x}", app.oracle.address()),
        addr = %cfg.bind,
        "countersigner listening - it holds a veto, never the funds"
    );
    match &app.guardian {
        Some(_) => tracing::info!("on-chain guardian enabled: revocations are written to the wallet"),
        None => tracing::warn!(
            "BASE_RPC unset: revocations stop countersigning but are not written on chain"
        ),
    }
    match &app.notify {
        Some(n) => tracing::info!(channel = %n.describe(), "phone push enabled"),
        None => tracing::info!("no phone push (NTFY_TOPIC unset) - approvals reach people through the approval page and the agent"),
    }
    if app.settings.control_token.is_none() {
        tracing::info!("CONTROL_TOKEN unset: the operator API (dashboard, revoke, restore) is disabled");
    }

    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    axum::serve(listener, api::router(app)).await?;
    Ok(())
}
