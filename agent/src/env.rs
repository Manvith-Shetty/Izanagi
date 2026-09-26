//! Every environment variable the agent reads, in one place.
//!
//! Note what is NOT here: no Intercepta key, no World client secret, no countersigning
//! key. The agent holds exactly one secret -- its own key -- and that key alone cannot
//! move money.

use common::utils::get_from_env_unsafe;

#[derive(Clone)]
pub struct AgentEnv {
    /// Where the countersigner lives. The agent asks; it does not decide.
    pub countersigner_url: String,
    /// The agent's hot key. Compromising it is not enough to spend anything.
    pub agent_private_key: String,
    /// The Countersign wallet that is the channel's `payer`.
    pub countersign_wallet: String,
    /// The seller's claim authorizer, for `agent pay`. `agent fetch` reads it from the seller's 402.
    pub receiver_authorizer: Option<String>,
    pub chain_id: u64,
    pub base_rpc: String,
}

impl std::fmt::Debug for AgentEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentEnv")
            .field("countersigner_url", &self.countersigner_url)
            .field("agent_private_key", &"<redacted>")
            .field("countersign_wallet", &self.countersign_wallet)
            .field("receiver_authorizer", &self.receiver_authorizer)
            .field("chain_id", &self.chain_id)
            .field("base_rpc", &self.base_rpc)
            .finish()
    }
}

impl AgentEnv {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            countersigner_url: get_from_env_unsafe("COUNTERSIGNER_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8787".to_string()),
            agent_private_key: get_from_env_unsafe("AGENT_PRIVATE_KEY")?,
            countersign_wallet: get_from_env_unsafe("COUNTERSIGN_WALLET")?,
            receiver_authorizer: get_from_env_unsafe("RECEIVER_AUTHORIZER").ok(),
            chain_id: get_from_env_unsafe("CHAIN_ID").unwrap_or(8453),
            base_rpc: get_from_env_unsafe("BASE_RPC")
                .unwrap_or_else(|_| "https://mainnet.base.org".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_keys_are_named_when_missing() {
        std::env::remove_var("AGENT_PRIVATE_KEY");
        let e = AgentEnv::new().unwrap_err();
        assert!(e.contains("AGENT_PRIVATE_KEY"), "must name the missing key: {e}");
    }

    #[test]
    fn debug_never_prints_the_agent_key() {
        let e = AgentEnv {
            countersigner_url: "http://x".into(),
            agent_private_key: "0xdeadbeefsecret".into(),
            countersign_wallet: "0xwallet".into(),
            receiver_authorizer: Some("0xauth".into()),
            chain_id: 8453,
            base_rpc: "https://mainnet.base.org".into(),
        };
        assert!(!format!("{e:?}").contains("deadbeefsecret"));
    }
}
