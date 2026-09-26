//! Every environment variable Tab reads, in one place.
//!
//! Tab is the product people and agents touch: the dashboard, the approval page, the MCP
//! server. Note what it does NOT hold: no Intercepta key, no World client secret, no
//! countersigning key, and no key that owns or funds anyone's wallet: people create and fund
//! their own. It holds the agent's key -- which alone cannot move money -- and the token that
//! lets it ask the countersigner to stop a payment, never to start one.

use alloy::primitives::Address;
use common::utils::get_from_env_unsafe;
use std::path::PathBuf;

#[derive(Clone)]
pub struct TabEnv {
    pub bind: String,
    /// The URL people reach Tab's website at: the session cookie and every page link belong to
    /// it. When the website is hosted elsewhere (Vercel) it proxies `/api` here.
    pub public_url: String,
    /// Where this server itself is reachable. MCP links point here directly, not through the
    /// website's proxy: an AI client holds its connection open for as long as it likes.
    pub api_url: String,
    pub countersigner_url: String,
    /// The countersigner's operator token: stop payments, ask a human to restart them.
    pub control_token: String,
    /// The agent's hot key. Signs vouchers (useless without a countersignature) and pays the
    /// gas to open tabs, which the wallet funds from its own balance through the collector.
    pub agent_private_key: String,
    /// The shared deposit collector every wallet opens tabs through (it only moves a wallet's
    /// own funds into channels that wallet gates, and only what the wallet approves per tab).
    pub collector: Address,
    pub chain_id: u64,
    pub rpc: String,
    /// Mainnet data for the network census, even when payments run on a fork.
    pub census_rpc: String,
    pub blockscout_api: String,
    /// Opening deposit for a new tab, atomic USDC.
    pub tab_deposit: u128,
    /// The most this agent will pay for a single call, whatever the seller asks.
    pub max_price: u128,
    pub web_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Our own shop for demonstrating the kill switch (the `seller` crate), if deployed.
    pub demo_shop_url: Option<String>,
}

impl std::fmt::Debug for TabEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabEnv")
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field("api_url", &self.api_url)
            .field("countersigner_url", &self.countersigner_url)
            .field("control_token", &"<redacted>")
            .field("agent_private_key", &"<redacted>")
            .field("collector", &self.collector)
            .field("chain_id", &self.chain_id)
            .field("rpc", &self.rpc)
            .field("tab_deposit", &self.tab_deposit)
            .field("max_price", &self.max_price)
            .field("web_dir", &self.web_dir)
            .field("state_dir", &self.state_dir)
            .field("demo_shop_url", &self.demo_shop_url)
            .finish()
    }
}

fn non_empty(k: &str) -> Option<String> {
    get_from_env_unsafe::<String>(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl TabEnv {
    pub fn new() -> Result<Self, String> {
        let bind: String = non_empty("TAB_BIND").unwrap_or_else(|| "127.0.0.1:3000".into());
        let public_url = non_empty("TAB_PUBLIC_URL")
            .unwrap_or_else(|| format!("http://{bind}"))
            .trim_end_matches('/')
            .to_string();
        if !(public_url.starts_with("https://") || public_url.starts_with("http://")) {
            return Err(format!("TAB_PUBLIC_URL must start with https:// or http:// (got {public_url:?})"));
        }
        let api_url = non_empty("TAB_API_URL").unwrap_or_else(|| public_url.clone()).trim_end_matches('/').to_string();
        if !(api_url.starts_with("https://") || api_url.starts_with("http://")) {
            return Err(format!("TAB_API_URL must start with https:// or http:// (got {api_url:?})"));
        }
        let secret = |k: &str| -> Result<Option<String>, String> {
            match non_empty(k) {
                Some(t) if t.len() < 16 => Err(format!("{k} must be at least 16 characters")),
                other => Ok(other),
            }
        };
        let collector = match non_empty("COUNTERSIGN_COLLECTOR") {
            Some(c) => Some(c.parse().map_err(|e| format!("COUNTERSIGN_COLLECTOR is not an address: {e}"))?),
            None => None,
        };
        let rpc = non_empty("BASE_RPC").unwrap_or_else(|| "https://mainnet.base.org".into());
        Ok(Self {
            public_url,
            api_url,
            bind,
            countersigner_url: non_empty("COUNTERSIGNER_URL")
                .unwrap_or_else(|| "http://127.0.0.1:8787".into())
                .trim_end_matches('/')
                .to_string(),
            control_token: secret("CONTROL_TOKEN")?.ok_or("CONTROL_TOKEN env not found: Tab needs the countersigner's operator token")?,
            agent_private_key: get_from_env_unsafe("AGENT_PRIVATE_KEY")?,
            collector: collector.ok_or("COUNTERSIGN_COLLECTOR env not found: every wallet approves one shared collector")?,
            chain_id: get_from_env_unsafe("CHAIN_ID").unwrap_or(8453),
            census_rpc: non_empty("CENSUS_RPC").unwrap_or_else(|| "https://mainnet.base.org".into()),
            rpc,
            blockscout_api: non_empty("BLOCKSCOUT_API").unwrap_or_else(|| "https://base.blockscout.com/api".into()),
            tab_deposit: get_from_env_unsafe("TAB_DEPOSIT").unwrap_or(50_000), // 0.05 USDC per shop
            max_price: get_from_env_unsafe("TAB_MAX_PRICE").unwrap_or(100_000), // 0.10 USDC a call
            web_dir: non_empty("TAB_WEB_DIR")
                .map(Into::into)
                .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/ui/dist").into()),
            state_dir: non_empty("TAB_STATE_DIR")
                .map(Into::into)
                .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/.state").into()),
            demo_shop_url: non_empty("DEMO_SHOP_URL").map(|u| u.trim_end_matches('/').to_string()),
        })
    }

    /// True when payments run against a local fork rather than the live chain.
    pub fn is_fork(&self) -> bool {
        self.rpc.contains("127.0.0.1") || self.rpc.contains("localhost")
    }

    /// The address a person's MCP client should be given. The token is theirs alone.
    pub fn mcp_url(&self, token: &str) -> String {
        format!("{}/mcp/{token}", self.api_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV: Mutex<()> = Mutex::new(());

    fn required() {
        std::env::set_var("CONTROL_TOKEN", "a-long-enough-control-token");
        std::env::set_var("AGENT_PRIVATE_KEY", "0x2");
        std::env::set_var("COUNTERSIGN_COLLECTOR", "0x1111111111111111111111111111111111111111");
        for k in ["TAB_PUBLIC_URL", "TAB_API_URL", "TAB_BIND"] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn required_keys_are_named_when_missing() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        std::env::remove_var("AGENT_PRIVATE_KEY");
        assert!(TabEnv::new().unwrap_err().contains("AGENT_PRIVATE_KEY"));
        required();
        std::env::remove_var("COUNTERSIGN_COLLECTOR");
        assert!(TabEnv::new().unwrap_err().contains("COUNTERSIGN_COLLECTOR"));
        required();
        std::env::remove_var("CONTROL_TOKEN");
        assert!(TabEnv::new().unwrap_err().contains("CONTROL_TOKEN"));
    }

    #[test]
    fn a_short_control_token_is_refused_by_name() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        std::env::set_var("CONTROL_TOKEN", "short");
        assert!(TabEnv::new().unwrap_err().contains("CONTROL_TOKEN"));
        required();
    }

    #[test]
    fn the_mcp_url_carries_a_persons_token_and_debug_hides_keys() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        std::env::set_var("TAB_PUBLIC_URL", "https://tab.example/");
        std::env::set_var("AGENT_PRIVATE_KEY", "0xagent-secret");
        let e = TabEnv::new().unwrap();
        assert_eq!(e.mcp_url("tok_123"), "https://tab.example/mcp/tok_123");
        std::env::set_var("TAB_API_URL", "https://api.tab.example/");
        let split = TabEnv::new().unwrap();
        assert_eq!(split.mcp_url("tok_123"), "https://api.tab.example/mcp/tok_123", "MCP goes straight to the server");
        assert_eq!(split.public_url, "https://tab.example");
        std::env::remove_var("TAB_API_URL");
        let d = format!("{e:?}");
        assert!(!d.contains("agent-secret") && !d.contains("a-long-enough-control-token"), "{d}");
        std::env::remove_var("TAB_PUBLIC_URL");
        required();
    }
}
