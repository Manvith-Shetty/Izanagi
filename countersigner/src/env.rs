//! Every environment variable this service reads, in one place.
//!
//! Nothing else in the crate touches `std::env`. If a key is missing or malformed the
//! process says so by name at startup rather than failing halfway through a payment.

use common::utils::get_from_env_unsafe;

/// Intercepta screening. There is deliberately no "disabled" variant: without a key the
/// policy engine fails CLOSED and refuses every payment. A guard that opens when its data
/// source is unavailable is not a guard.
#[derive(Clone)]
pub struct InterceptaEnv {
    pub api_key: String,
}

impl InterceptaEnv {
    pub fn new() -> Result<Self, String> {
        // absent is allowed at startup so the service still boots and explains itself;
        // the policy engine then refuses everything.
        let api_key: String = get_from_env_unsafe("INTERCEPTA_API_KEY").unwrap_or_default();
        if api_key.trim().is_empty() {
            tracing::warn!(
                "INTERCEPTA_API_KEY is not set - the policy engine will FAIL CLOSED and refuse \
                 every payment. Free key: https://intercepta.io/ethglobal"
            );
        }
        Ok(Self { api_key })
    }
}

/// World ID for Agents. Absent credentials disable the human-approval path, which means
/// any verdict of `ask` becomes an effective refusal rather than a silent approval.
#[derive(Clone)]
pub enum WorldEnv {
    Enabled {
        issuer: String,
        client_id: String,
        client_secret: String,
        require_orb: bool,
    },
    Disabled,
}

impl WorldEnv {
    pub fn new() -> Result<Self, String> {
        let client_id: String = match get_from_env_unsafe("WORLD_CLIENT_ID") {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("WORLD_CLIENT_ID not set - the human-approval path is disabled");
                return Ok(WorldEnv::Disabled);
            }
        };
        let client_secret: String = get_from_env_unsafe("WORLD_CLIENT_SECRET")
            .map_err(|e| format!("WORLD_CLIENT_ID is set but {e}"))?;
        let issuer: String = get_from_env_unsafe("WORLD_ISSUER")
            .unwrap_or_else(|_| "https://sandbox.auth.world.org".to_string());
        // orb-level proof of personhood, not merely "someone logged in"
        let require_orb: bool = get_from_env_unsafe("WORLD_REQUIRE_ORB").unwrap_or(true);

        Ok(WorldEnv::Enabled {
            issuer,
            client_id,
            client_secret,
            require_orb,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_enabled(&self) -> bool {
        matches!(self, WorldEnv::Enabled { .. })
    }
}

/// Phone push over ntfy, used to exercise the human-approval path without watching
/// a terminal. Purely observational: unlike `InterceptaEnv`, absence is not a refusal
/// and an unreachable broker is not a denial. See `notify.rs`.
#[derive(Clone)]
pub enum NotifyEnv {
    Enabled {
        server: String,
        /// Becomes a URL path segment, so its charset is validated here.
        topic: String,
        /// Only needed for a protected or self-hosted topic.
        token: Option<String>,
        /// Notification avatar. ntfy renders PNG and JPEG only, so an `.ico` is
        /// silently ignored by the phone app.
        icon: Option<String>,
    },
    Disabled,
}

impl NotifyEnv {
    pub fn new() -> Result<Self, String> {
        let topic: String = match get_from_env_unsafe("NTFY_TOPIC") {
            Ok(v) => v,
            Err(_) => return Ok(NotifyEnv::Disabled),
        };
        let topic = topic.trim().to_string();
        if topic.is_empty() {
            return Ok(NotifyEnv::Disabled);
        }
        // The topic is interpolated into a URL. Reject anything that could escape the
        // path segment rather than trusting the broker to reject it.
        if !topic
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            return Err(
                "NTFY_TOPIC may contain only letters, digits, '_' and '-'".to_string()
            );
        }

        let server: String = get_from_env_unsafe("NTFY_SERVER")
            .unwrap_or_else(|_| "https://ntfy.sh".to_string());
        let server = server.trim().trim_end_matches('/').to_string();
        if !(server.starts_with("https://") || server.starts_with("http://")) {
            return Err(format!(
                "NTFY_SERVER must start with https:// or http:// (got {server:?})"
            ));
        }

        let token: Option<String> = get_from_env_unsafe("NTFY_TOKEN")
            .ok()
            .map(|t: String| t.trim().to_string())
            .filter(|t| !t.is_empty());

        // A recognisable avatar is most of what makes a push look deliberate rather
        // than like a cron job. Set NTFY_ICON= (empty) to send none.
        let icon: String = get_from_env_unsafe("NTFY_ICON")
            .unwrap_or_else(|_| "https://github.com/worldcoin.png?size=200".to_string());
        let icon = Some(icon.trim().to_string()).filter(|i| !i.is_empty());

        Ok(NotifyEnv::Enabled { server, topic, token, icon })
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, NotifyEnv::Enabled { .. })
    }
}

/// Thresholds for the pay / cap / ask / refuse decision. All USDC amounts are atomic
/// (6 decimals), so 20_000_000 is 20 USDC.
#[derive(Clone, Debug)]
pub struct PolicyEnv {
    /// Below this, a clean counterparty is paid with no human involved.
    pub autonomous_limit: u128,
    /// Per-PERSON budget, keyed by the OIDC pairwise `sub`, shared across their agents.
    pub human_limit: u128,
    /// How long a countersignature stays valid. Short, so declining to re-issue is itself
    /// a revocation.
    pub attestation_ttl: u64,
    /// How often open sessions are re-screened.
    pub rescreen_secs: u64,
    pub refuse_at: f64,
    pub ask_at: f64,
    pub cap_at: f64,
    pub capped_ceiling: u128,
    /// Reject prices far above the live x402 Bazaar distribution (median there: 0.01 USDC).
    pub max_price: u128,
    /// How long a seller's score is reused between payments. The watcher always asks fresh.
    pub address_cache_secs: u64,
    /// How long a token's trust verdict is reused. Token intelligence moves slowly.
    pub token_cache_secs: u64,
    /// When a tab needs a person, they approve raising its limit to the next multiple of
    /// this -- not one exact voucher -- so the agent is not stopped again on the next call.
    /// 0 => the autonomous limit.
    pub approval_step: u128,
}

impl PolicyEnv {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            autonomous_limit: get_from_env_unsafe("AUTONOMOUS_LIMIT").unwrap_or(20_000_000),
            human_limit: get_from_env_unsafe("HUMAN_LIMIT").unwrap_or(500_000_000),
            attestation_ttl: get_from_env_unsafe("ATTESTATION_TTL").unwrap_or(120),
            // every re-screen is one metered Intercepta call per open seller
            rescreen_secs: get_from_env_unsafe("RESCREEN_SECS").unwrap_or(60),
            address_cache_secs: get_from_env_unsafe("ADDRESS_CACHE_SECS").unwrap_or(30),
            token_cache_secs: get_from_env_unsafe("TOKEN_CACHE_SECS").unwrap_or(3600),
            approval_step: get_from_env_unsafe("APPROVAL_STEP").unwrap_or(0),
            refuse_at: get_from_env_unsafe("REFUSE_AT").unwrap_or(60.0),
            ask_at: get_from_env_unsafe("ASK_AT").unwrap_or(30.0),
            cap_at: get_from_env_unsafe("CAP_AT").unwrap_or(10.0),
            capped_ceiling: get_from_env_unsafe("CAPPED_CEILING").unwrap_or(1_000_000),
            max_price: get_from_env_unsafe("MAX_PRICE").unwrap_or(100_000_000),
        })
    }
}

impl std::fmt::Debug for InterceptaEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterceptaEnv")
            .field("api_key", &redact(&self.api_key))
            .finish()
    }
}

impl std::fmt::Debug for WorldEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorldEnv::Disabled => f.write_str("WorldEnv::Disabled"),
            WorldEnv::Enabled { issuer, client_id, client_secret, require_orb } => f
                .debug_struct("WorldEnv::Enabled")
                .field("issuer", issuer)
                .field("client_id", client_id)
                .field("client_secret", &redact(client_secret))
                .field("require_orb", require_orb)
                .finish(),
        }
    }
}

impl std::fmt::Debug for NotifyEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifyEnv::Disabled => f.write_str("NotifyEnv::Disabled"),
            // The topic is a bearer capability: knowing it is enough to read every
            // message and publish more. It is a secret, so it is redacted like one.
            NotifyEnv::Enabled { server, topic, token, icon } => f
                .debug_struct("NotifyEnv::Enabled")
                .field("server", server)
                .field("topic", &redact(topic))
                .field("token", &redact(token.as_deref().unwrap_or("")))
                .field("icon", icon)
                .finish(),
        }
    }
}

/// Never print a secret, but do say whether one is present.
fn redact(v: &str) -> &'static str {
    if v.trim().is_empty() { "<unset>" } else { "<redacted>" }
}

/// Where the countersigner talks to the rest of the product.
#[derive(Clone)]
pub struct ServiceEnv {
    /// Chain RPC for the on-chain kill switch. Unset => revocations stop countersigning (and
    /// outstanding attestations lapse within `ATTESTATION_TTL`) but nothing is written on chain.
    pub rpc: Option<String>,
    /// Bearer token for the operator API: revoke, restore, activity, screening. Unset => that
    /// API does not exist. The agent-facing endpoints never need it.
    pub control_token: Option<String>,
    /// Durable state: which human each wallet is bound to.
    pub state_dir: std::path::PathBuf,
    /// Base URL of the page that shows a person what they are approving, e.g.
    /// `https://tab.example`. Pushes link to `{url}/approve/{id}`; unset => World's own URL.
    pub approval_page_url: Option<String>,
}

impl ServiceEnv {
    pub fn new() -> Result<Self, String> {
        let non_empty = |k: &str| -> Option<String> {
            get_from_env_unsafe::<String>(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
        };
        let control_token = non_empty("CONTROL_TOKEN");
        if control_token.as_ref().is_some_and(|t| t.len() < 16) {
            return Err("CONTROL_TOKEN must be at least 16 characters".into());
        }
        let approval_page_url = non_empty("APPROVAL_PAGE_URL").map(|u| u.trim_end_matches('/').to_string());
        if let Some(u) = &approval_page_url {
            if !(u.starts_with("https://") || u.starts_with("http://")) {
                return Err(format!("APPROVAL_PAGE_URL must start with https:// or http:// (got {u:?})"));
            }
        }
        Ok(Self {
            rpc: non_empty("BASE_RPC"),
            control_token,
            state_dir: non_empty("STATE_DIR")
                .map(Into::into)
                .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/.state").into()),
            approval_page_url,
        })
    }
}

impl std::fmt::Debug for ServiceEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceEnv")
            .field("rpc", &self.rpc)
            .field("control_token", &redact(self.control_token.as_deref().unwrap_or("")))
            .field("state_dir", &self.state_dir)
            .field("approval_page_url", &self.approval_page_url)
            .finish()
    }
}

/// The whole service configuration.
#[derive(Clone)]
pub struct CountersignerEnv {
    pub bind: String,
    /// Holds a veto, never the funds: it cannot move, redirect or trap money.
    pub oracle_private_key: String,
    pub intercepta: InterceptaEnv,
    pub world: WorldEnv,
    /// Observability only. Nothing here can approve, deny or cap a payment.
    pub notify: NotifyEnv,
    pub policy: PolicyEnv,
    pub service: ServiceEnv,
}

impl std::fmt::Debug for CountersignerEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CountersignerEnv")
            .field("bind", &self.bind)
            .field("oracle_private_key", &redact(&self.oracle_private_key))
            .field("intercepta", &self.intercepta)
            .field("world", &self.world)
            .field("notify", &self.notify)
            .field("policy", &self.policy)
            .field("service", &self.service)
            .finish()
    }
}

impl CountersignerEnv {
    pub fn new() -> Result<Self, String> {
        // the only genuinely required value: without it there is nothing to countersign with
        let oracle_private_key: String = get_from_env_unsafe("ORACLE_PRIVATE_KEY")?;
        let bind: String =
            get_from_env_unsafe("BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

        Ok(Self {
            bind,
            oracle_private_key,
            intercepta: InterceptaEnv::new()?,
            world: WorldEnv::new()?,
            // A malformed topic is refused at startup, by name, rather than silently
            // sending nothing: no payment is in flight yet, so nothing is gated by it.
            notify: NotifyEnv::new()?,
            policy: PolicyEnv::new()?,
            service: ServiceEnv::new()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_falls_back_to_sane_defaults() {
        for k in [
            "AUTONOMOUS_LIMIT", "HUMAN_LIMIT", "ATTESTATION_TTL", "RESCREEN_SECS",
            "REFUSE_AT", "ASK_AT", "CAP_AT", "CAPPED_CEILING", "MAX_PRICE",
            "ADDRESS_CACHE_SECS", "TOKEN_CACHE_SECS", "APPROVAL_STEP",
        ] {
            std::env::remove_var(k);
        }
        let p = PolicyEnv::new().unwrap();
        assert_eq!(p.autonomous_limit, 20_000_000, "20 USDC");
        assert!(p.address_cache_secs < p.rescreen_secs, "a cached score must be younger than a re-screen");
        assert_eq!(p.attestation_ttl, 120, "short TTL is load-bearing for revocation");
        assert!(p.cap_at < p.ask_at && p.ask_at < p.refuse_at, "thresholds must be ordered");
    }

    /// One test, because these all mutate process-global environment state.
    #[test]
    fn required_and_optional_keys_behave() {
        // World disabled when no client id
        std::env::remove_var("WORLD_CLIENT_ID");
        std::env::remove_var("WORLD_CLIENT_SECRET");
        assert!(!WorldEnv::new().unwrap().is_enabled());

        // a client id without a secret is a misconfiguration, and says which key
        std::env::set_var("WORLD_CLIENT_ID", "app_test");
        let e = WorldEnv::new().unwrap_err();
        assert!(e.contains("WORLD_CLIENT_SECRET"), "must name the missing key: {e}");
        std::env::remove_var("WORLD_CLIENT_ID");

        // the oracle key is the one genuinely required value
        std::env::remove_var("ORACLE_PRIVATE_KEY");
        let e = CountersignerEnv::new().unwrap_err();
        assert!(e.contains("ORACLE_PRIVATE_KEY"), "must name the missing key: {e}");
    }

    #[test]
    fn notify_is_off_by_default_and_validates_its_inputs() {
        for k in ["NTFY_TOPIC", "NTFY_SERVER", "NTFY_TOKEN"] {
            std::env::remove_var(k);
        }
        assert!(!NotifyEnv::new().unwrap().is_enabled(), "absent topic => disabled");

        // whitespace-only is the same as unset, not a topic named " "
        std::env::set_var("NTFY_TOPIC", "   ");
        assert!(!NotifyEnv::new().unwrap().is_enabled());

        // a topic that would escape its URL path segment is refused by name
        std::env::set_var("NTFY_TOPIC", "evil/../admin");
        assert!(NotifyEnv::new().unwrap_err().contains("NTFY_TOPIC"));

        std::env::set_var("NTFY_TOPIC", "countersign-9f3c2a");
        match NotifyEnv::new().unwrap() {
            NotifyEnv::Enabled { server, topic, token, icon } => {
                assert_eq!(server, "https://ntfy.sh", "default broker");
                assert_eq!(topic, "countersign-9f3c2a");
                assert!(token.is_none());
                assert!(icon.is_some(), "a default avatar ships with it");
            }
            NotifyEnv::Disabled => panic!("should be enabled"),
        }

        // a trailing slash would produce ntfy.sh//topic
        std::env::set_var("NTFY_SERVER", "https://push.example.com/");
        match NotifyEnv::new().unwrap() {
            NotifyEnv::Enabled { server, .. } => {
                assert_eq!(server, "https://push.example.com")
            }
            NotifyEnv::Disabled => panic!("should be enabled"),
        }

        std::env::set_var("NTFY_SERVER", "ftp://push.example.com");
        assert!(NotifyEnv::new().unwrap_err().contains("NTFY_SERVER"));

        for k in ["NTFY_TOPIC", "NTFY_SERVER", "NTFY_TOKEN"] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn service_settings_validate_their_inputs() {
        for k in ["BASE_RPC", "CONTROL_TOKEN", "STATE_DIR", "APPROVAL_PAGE_URL"] {
            std::env::remove_var(k);
        }
        let s = ServiceEnv::new().unwrap();
        assert!(s.rpc.is_none(), "no RPC => no on-chain guardian");
        assert!(s.control_token.is_none(), "no token => no operator API");
        assert!(s.state_dir.ends_with(".state"));

        std::env::set_var("CONTROL_TOKEN", "short");
        assert!(ServiceEnv::new().unwrap_err().contains("CONTROL_TOKEN"));
        std::env::set_var("CONTROL_TOKEN", "a-long-enough-control-token");
        std::env::set_var("APPROVAL_PAGE_URL", "tab.example");
        assert!(ServiceEnv::new().unwrap_err().contains("APPROVAL_PAGE_URL"));
        std::env::set_var("APPROVAL_PAGE_URL", "https://tab.example/");
        let s = ServiceEnv::new().unwrap();
        assert_eq!(s.approval_page_url.as_deref(), Some("https://tab.example"), "trailing slash trimmed");
        assert!(!format!("{s:?}").contains("a-long-enough-control-token"), "token leaked into Debug");

        for k in ["CONTROL_TOKEN", "APPROVAL_PAGE_URL"] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn debug_never_prints_secrets() {
        let w = WorldEnv::Enabled {
            issuer: "https://sandbox.auth.world.org".into(),
            client_id: "app_public".into(),
            client_secret: "super-secret-value".into(),
            require_orb: true,
        };
        let s = format!("{w:?}");
        assert!(!s.contains("super-secret-value"), "client secret leaked into Debug");
        assert!(s.contains("<redacted>"));
        assert!(s.contains("app_public"), "the public client id is fine to show");

        let i = InterceptaEnv { api_key: String::new() };
        assert!(format!("{i:?}").contains("<unset>"));

        let n = NotifyEnv::Enabled {
            server: "https://ntfy.sh".into(),
            topic: "countersign-9f3c2a".into(),
            token: Some("tk_secret".into()),
            icon: None,
        };
        let s = format!("{n:?}");
        assert!(!s.contains("countersign-9f3c2a"), "ntfy topic leaked into Debug");
        assert!(!s.contains("tk_secret"), "ntfy token leaked into Debug");
        assert!(s.contains("https://ntfy.sh"), "the broker host is fine to show");
    }
}
