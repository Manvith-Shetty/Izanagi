//! Every environment variable the seller reads, in one place.

use alloy::primitives::Address;
use common::utils::get_from_env_unsafe;

#[derive(Clone)]
pub struct SellerEnv {
    pub bind: String,
    /// Base URL clients reach us at, advertised in the 402's `resource`.
    pub public_url: String,
    /// Where settled funds land (`payTo`).
    pub receiver: Address,
    /// The key that authorises claims, i.e. the seller cashing vouchers in. Also pays the gas.
    pub receiver_authorizer_key: String,
    /// Maximum price per request, atomic USDC (6 decimals).
    pub price: u128,
    pub token: Address,
    /// EIP-712 domain of the token contract, advertised for deposits.
    pub token_name: String,
    pub token_version: String,
    pub withdraw_delay: u64,
    pub chain_id: u64,
    pub rpc: String,
    /// Screens every payer. Unset => every payer is refused: no mock mode, same as the countersigner.
    pub intercepta_api_key: String,
    pub screen: ScreenConfig,
    pub claim: ClaimConfig,
    /// Bearer token for `POST /admin/claim`. Unset => the endpoint does not exist.
    pub admin_token: Option<String>,
    /// Serve junk from this paid call onwards on each channel (the demo shop goes bad). 0 = never.
    pub rogue_after: u64,
}

/// How far to trust a payer, by Intercepta toxicScore.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenConfig {
    /// At or above: refuse to serve.
    pub refuse_at: f64,
    /// At or above: serve, but extend no credit -- claim after every request.
    pub careful_at: f64,
    /// How long one payer verdict is reused.
    pub cache_secs: u64,
}

/// When to cash vouchers in.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimConfig {
    /// How often the claim loop looks at open channels.
    pub tick_secs: u64,
    /// Claim this long before a Countersign attestation lapses.
    pub margin_secs: u64,
    /// Unclaimed value we will carry for a trusted payer before claiming.
    pub max_unclaimed: u128,
    /// Claim anything left unclaimed this long, whatever else holds.
    pub max_age_secs: u64,
}

impl std::fmt::Debug for SellerEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SellerEnv")
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field("receiver", &self.receiver)
            .field("receiver_authorizer_key", &"<redacted>")
            .field("price", &self.price)
            .field("token", &self.token)
            .field("withdraw_delay", &self.withdraw_delay)
            .field("chain_id", &self.chain_id)
            .field("rpc", &self.rpc)
            .field("intercepta_api_key", &"<redacted>")
            .field("screen", &self.screen)
            .field("claim", &self.claim)
            .field("admin_token", &self.admin_token.as_ref().map(|_| "<redacted>"))
            .field("rogue_after", &self.rogue_after)
            .finish()
    }
}

impl SellerEnv {
    pub fn new() -> Result<Self, String> {
        let bind: String =
            get_from_env_unsafe("SELLER_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let screen = ScreenConfig {
            refuse_at: get_from_env_unsafe("PAYER_REFUSE_AT").unwrap_or(60.0),
            careful_at: get_from_env_unsafe("PAYER_CAREFUL_AT").unwrap_or(30.0),
            cache_secs: get_from_env_unsafe("PAYER_SCREEN_CACHE_SECS").unwrap_or(60),
        };
        if screen.careful_at > screen.refuse_at {
            return Err("PAYER_CAREFUL_AT must not exceed PAYER_REFUSE_AT".into());
        }
        Ok(Self {
            public_url: get_from_env_unsafe("SELLER_PUBLIC_URL")
                .unwrap_or_else(|_| format!("http://{bind}")),
            bind,
            receiver: get_from_env_unsafe("SELLER_RECEIVER")?,
            receiver_authorizer_key: get_from_env_unsafe("SELLER_AUTHORIZER_KEY")?,
            price: get_from_env_unsafe("SELLER_PRICE").unwrap_or(10_000), // 0.01 USDC, the live Bazaar median
            token: get_from_env_unsafe("SELLER_TOKEN").unwrap_or(common::USDC_BASE),
            // what USDC's own eip712Domain() reports on Base
            token_name: get_from_env_unsafe("SELLER_TOKEN_NAME").unwrap_or_else(|_| "USD Coin".into()),
            token_version: get_from_env_unsafe("SELLER_TOKEN_VERSION").unwrap_or_else(|_| "2".into()),
            withdraw_delay: get_from_env_unsafe("SELLER_WITHDRAW_DELAY").unwrap_or(common::MIN_WITHDRAW_DELAY),
            chain_id: get_from_env_unsafe("CHAIN_ID").unwrap_or(8453),
            rpc: get_from_env_unsafe("BASE_RPC")
                .unwrap_or_else(|_| "https://mainnet.base.org".to_string()),
            intercepta_api_key: get_from_env_unsafe("INTERCEPTA_API_KEY").unwrap_or_default(),
            screen,
            claim: ClaimConfig {
                tick_secs: get_from_env_unsafe("CLAIM_TICK_SECS").unwrap_or(10),
                margin_secs: get_from_env_unsafe("CLAIM_MARGIN_SECS").unwrap_or(30),
                max_unclaimed: get_from_env_unsafe("CLAIM_MAX_UNCLAIMED").unwrap_or(1_000_000),
                max_age_secs: get_from_env_unsafe("CLAIM_MAX_AGE_SECS").unwrap_or(3600),
            },
            admin_token: get_from_env_unsafe::<String>("SELLER_ADMIN_TOKEN")
                .ok()
                .filter(|t| !t.trim().is_empty()),
            rogue_after: get_from_env_unsafe("SELLER_ROGUE_AFTER").unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The process environment is shared by every test thread.
    static ENV: Mutex<()> = Mutex::new(());

    fn required() {
        std::env::set_var("SELLER_RECEIVER", "0x2222222222222222222222222222222222222222");
        std::env::set_var("SELLER_AUTHORIZER_KEY", "0x2");
    }

    #[test]
    fn price_defaults_to_the_live_bazaar_median() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("SELLER_PRICE");
        required();
        assert_eq!(SellerEnv::new().unwrap().price, 10_000, "0.01 USDC");
    }

    #[test]
    fn receiver_must_be_an_address() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        std::env::set_var("SELLER_RECEIVER", "0x1");
        let e = SellerEnv::new().unwrap_err();
        assert!(e.contains("SELLER_RECEIVER"), "must name the bad key: {e}");
        required();
    }

    #[test]
    fn debug_never_prints_secrets() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        required();
        let mut e = SellerEnv::new().unwrap();
        e.receiver_authorizer_key = "0xdeadbeefsecret".into();
        e.intercepta_api_key = "interceptasecret".into();
        e.admin_token = Some("admintokensecret".into());
        let s = format!("{e:?}");
        assert!(!s.contains("deadbeefsecret") && !s.contains("interceptasecret") && !s.contains("admintokensecret"));
    }
}
