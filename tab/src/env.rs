//! Every environment variable Tab reads, in one place.
//!
//! Tab is the product people and agents touch: the dashboard, the approval page, the MCP
//! server. Note what it does NOT hold: no Intercepta key, no World client secret, no
//! countersigning key. It holds the agent's key -- which alone cannot move money -- and the
//! token that lets it ask the countersigner to stop a payment, never to start one.

use alloy::primitives::Address;
use common::utils::get_from_env_unsafe;
use std::path::PathBuf;

#[derive(Clone)]
pub struct TabEnv {
    pub bind: String,
    /// The URL people reach Tab at. Approval links and the MCP address are built from it.
    pub public_url: String,
    pub countersigner_url: String,
    /// The countersigner's operator token: stop payments, ask a human to restart them.
    pub control_token: String,
    /// The agent's hot key. Signs vouchers (useless without a countersignature) and pays the
    /// gas to open tabs from the wallet's pre-approved allowance.
    pub agent_private_key: String,
    /// The Countersign wallet every tab is paid from.
    pub wallet: Address,
    /// The wallet's deposit collector. With it, Tab opens tabs itself, within the allowance
    /// the owner granted; without it, tabs must be opened by the owner.
    pub collector: Option<Address>,
    pub chain_id: u64,
    pub rpc: String,
    /// Mainnet data for the network census, even when payments run on a fork.
    pub census_rpc: String,
    pub blockscout_api: String,
    /// Opening deposit for a new tab, atomic USDC.
    pub tab_deposit: u128,
    /// The most this agent will pay for a single call, whatever the seller asks.
    pub max_price: u128,
    /// Bearer token for the dashboard's buttons. Unset => the dashboard is read-only.
    pub admin_token: Option<String>,
    /// Secret for the MCP endpoint (`/mcp/{token}` or `Authorization: Bearer`). Unset => the
    /// MCP server is open to anyone who can reach it, which spends real money: only for local use.
    pub mcp_token: Option<String>,
    pub web_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl std::fmt::Debug for TabEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let set = |o: &Option<String>| if o.is_some() { "<redacted>" } else { "<unset>" };
        f.debug_struct("TabEnv")
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field("countersigner_url", &self.countersigner_url)
            .field("control_token", &"<redacted>")
            .field("agent_private_key", &"<redacted>")
            .field("wallet", &self.wallet)
            .field("collector", &self.collector)
            .field("chain_id", &self.chain_id)
            .field("rpc", &self.rpc)
            .field("tab_deposit", &self.tab_deposit)
            .field("max_price", &self.max_price)
            .field("admin_token", &set(&self.admin_token))
            .field("mcp_token", &set(&self.mcp_token))
            .field("web_dir", &self.web_dir)
            .field("state_dir", &self.state_dir)
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
            bind,
            countersigner_url: non_empty("COUNTERSIGNER_URL")
                .unwrap_or_else(|| "http://127.0.0.1:8787".into())
                .trim_end_matches('/')
                .to_string(),
            control_token: secret("CONTROL_TOKEN")?.ok_or("CONTROL_TOKEN env not found: Tab needs the countersigner's operator token")?,
            agent_private_key: get_from_env_unsafe("AGENT_PRIVATE_KEY")?,
            wallet: get_from_env_unsafe("COUNTERSIGN_WALLET")?,
            collector,
            chain_id: get_from_env_unsafe("CHAIN_ID").unwrap_or(8453),
            census_rpc: non_empty("CENSUS_RPC").unwrap_or_else(|| "https://mainnet.base.org".into()),
            rpc,
            blockscout_api: non_empty("BLOCKSCOUT_API").unwrap_or_else(|| "https://base.blockscout.com/api".into()),
            tab_deposit: get_from_env_unsafe("TAB_DEPOSIT").unwrap_or(1_000_000), // 1 USDC
            max_price: get_from_env_unsafe("TAB_MAX_PRICE").unwrap_or(100_000), // 0.10 USDC a call
            admin_token: secret("TAB_ADMIN_TOKEN")?,
            mcp_token: secret("TAB_MCP_TOKEN")?,
            web_dir: non_empty("TAB_WEB_DIR")
                .map(Into::into)
                .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/web/dist").into()),
            state_dir: non_empty("TAB_STATE_DIR")
                .map(Into::into)
                .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/.state").into()),
        })
    }

    /// True when payments run against a local fork rather than the live chain.
    pub fn is_fork(&self) -> bool {
        self.rpc.contains("127.0.0.1") || self.rpc.contains("localhost")
    }

    /// The address an MCP client should be given.
    pub fn mcp_url(&self) -> String {
        match &self.mcp_token {
            Some(t) => format!("{}/mcp/{t}", self.public_url),
            None => format!("{}/mcp", self.public_url),
        }
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
        std::env::set_var("COUNTERSIGN_WALLET", "0x1111111111111111111111111111111111111111");
        for k in ["TAB_PUBLIC_URL", "TAB_ADMIN_TOKEN", "TAB_MCP_TOKEN", "COUNTERSIGN_COLLECTOR", "TAB_BIND"] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn required_keys_are_named_when_missing() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        std::env::remove_var("COUNTERSIGN_WALLET");
        assert!(TabEnv::new().unwrap_err().contains("COUNTERSIGN_WALLET"));
        required();
        std::env::remove_var("CONTROL_TOKEN");
        assert!(TabEnv::new().unwrap_err().contains("CONTROL_TOKEN"));
    }

    #[test]
    fn short_secrets_are_refused_by_name() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        std::env::set_var("TAB_MCP_TOKEN", "short");
        assert!(TabEnv::new().unwrap_err().contains("TAB_MCP_TOKEN"));
        std::env::remove_var("TAB_MCP_TOKEN");
    }

    #[test]
    fn the_mcp_url_carries_its_secret_and_debug_does_not() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        std::env::set_var("TAB_PUBLIC_URL", "https://tab.example/");
        std::env::set_var("TAB_MCP_TOKEN", "mcp-secret-0123456789");
        let e = TabEnv::new().unwrap();
        assert_eq!(e.mcp_url(), "https://tab.example/mcp/mcp-secret-0123456789");
        let d = format!("{e:?}");
        assert!(!d.contains("mcp-secret") && !d.contains("a-long-enough-control-token"), "{d}");
        std::env::remove_var("TAB_MCP_TOKEN");
        std::env::remove_var("TAB_PUBLIC_URL");
    }
}
