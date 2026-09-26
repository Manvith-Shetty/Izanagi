//! Tab -- the website, the dashboard API and the MCP server.

use anyhow::Result;
use tab::{api, app::App, env::TabEnv};

#[tokio::main]
async fn main() -> Result<()> {
    // this crate's own .env (wherever cargo was invoked from), then ./.env; refuses to
    // start on a file the parser would only half-read -- see common::utils::load_env
    common::utils::load_env(env!("CARGO_MANIFEST_DIR")).map_err(anyhow::Error::msg)?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "tab=info,rmcp=warn".into()),
        )
        .init();

    let env = TabEnv::new().map_err(anyhow::Error::msg)?;
    let bind = env.bind.clone();
    let app = App::new(env)?;

    tokio::spawn(app.brain.clone().follow(app.feed.clone()));
    tokio::spawn(app.clone().watch_claims());
    let census = app.census.clone();
    tokio::spawn(async move { census.run().await });

    tracing::info!(
        public = %app.env.public_url,
        agent = %app.chain.agent_address,
        treasury = %app.chain.treasury_address,
        "Tab listening on http://{bind}"
    );
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, api::router(app)).await?;
    Ok(())
}
