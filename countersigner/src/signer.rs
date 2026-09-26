//! The countersigning key.
//!
//! This key holds a VETO, never the funds. On its own it can do nothing:
//!   - it cannot move money    (the agent must also sign the same voucher digest)
//!   - it cannot redirect it   (the digest binds the channelId, which binds the receiver)
//!   - it cannot trap it       (withdrawals are owner-only and need no oracle)
//!
//! Its only power is to decline, and to revoke a seller before settlement.

use alloy::primitives::Address;
use alloy::signers::{local::PrivateKeySigner, SignerSync};
use anyhow::Result;
use common::{attestation_hash, channel_id, voucher_digest, ChannelConfig};

#[derive(Clone)]
pub struct OracleSigner {
    signer: PrivateKeySigner,
}

/// What the agent gets back when a payment is approved. camelCase, like the rest of the API.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Countersignature {
    /// Hex `0x..` of the 65-byte oracle signature over the attestation.
    pub oracle_signature: String,
    /// The voucher digest that was countersigned (the agent must sign this same hash).
    pub digest: String,
    /// The channel this is valid for.
    pub channel_id: String,
    /// The ceiling actually authorised, which may be lower than requested (a `cap`).
    pub ceiling: u128,
    /// Seller bound into the attestation.
    pub seller: String,
    /// Unix seconds. Kept short so that declining to re-issue is itself a revocation.
    pub expiry: u64,
}

impl OracleSigner {
    pub fn from_hex_key(key: &str) -> Result<Self> {
        let signer: PrivateKeySigner = key.trim().trim_start_matches("0x").parse()?;
        Ok(Self { signer })
    }

    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Countersign one voucher ceiling for one seller.
    ///
    /// The attestation binds (digest, seller, expiry). It cannot be replayed onto a different
    /// channel or a different amount, because both are inside `digest`.
    pub fn countersign(
        &self,
        cfg: &ChannelConfig,
        chain_id: u64,
        ceiling: u128,
        expiry: u64,
    ) -> Result<Countersignature> {
        let cid = channel_id(cfg, chain_id);
        let digest = voucher_digest(cid, ceiling, chain_id);
        let att = attestation_hash(digest, cfg.receiver, expiry);
        let sig = self.signer.sign_hash_sync(&att)?;

        Ok(Countersignature {
            oracle_signature: hex_bytes(&sig.as_bytes()),
            digest: format!("{digest:#x}"),
            channel_id: format!("{cid:#x}"),
            ceiling,
            seller: format!("{:#x}", cfg.receiver),
            expiry,
        })
    }

}

pub fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(2 + b.len() * 2);
    s.push_str("0x");
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};
    use common::USDC_BASE;

    const TEST_KEY: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

    fn cfg(seller: Address) -> ChannelConfig {
        ChannelConfig::countersign(
            address!("1111111111111111111111111111111111111111"),
            seller,
            address!("3333333333333333333333333333333333333333"),
            USDC_BASE,
            900,
            b256!("0000000000000000000000000000000000000000000000000000000000000001"),
        )
    }

    #[test]
    fn countersignature_is_65_bytes_and_recovers_to_the_oracle() {
        let s = OracleSigner::from_hex_key(TEST_KEY).unwrap();
        let seller = address!("2222222222222222222222222222222222222222");
        let c = s.countersign(&cfg(seller), 8453, 5_000_000, 2_000_000_000).unwrap();
        // 0x + 130 hex chars = 65 bytes
        assert_eq!(c.oracle_signature.len(), 132, "65-byte signature");
        assert_eq!(c.seller, format!("{seller:#x}"));
        assert_eq!(c.ceiling, 5_000_000);
    }

    #[test]
    fn different_sellers_produce_different_attestations() {
        let s = OracleSigner::from_hex_key(TEST_KEY).unwrap();
        let a = s
            .countersign(&cfg(address!("2222222222222222222222222222222222222222")), 8453, 5_000_000, 2_000_000_000)
            .unwrap();
        let b = s
            .countersign(&cfg(address!("4444444444444444444444444444444444444444")), 8453, 5_000_000, 2_000_000_000)
            .unwrap();
        assert_ne!(a.oracle_signature, b.oracle_signature);
        assert_ne!(a.channel_id, b.channel_id, "seller is part of channel identity");
    }

    #[test]
    fn different_ceilings_produce_different_digests() {
        let s = OracleSigner::from_hex_key(TEST_KEY).unwrap();
        let c = cfg(address!("2222222222222222222222222222222222222222"));
        let a = s.countersign(&c, 8453, 5_000_000, 2_000_000_000).unwrap();
        let b = s.countersign(&c, 8453, 6_000_000, 2_000_000_000).unwrap();
        assert_eq!(a.channel_id, b.channel_id);
        assert_ne!(a.digest, b.digest, "ceiling is inside the voucher digest");
    }

    #[test]
    fn digest_matches_live_base_mainnet_contract() {
        let s = OracleSigner::from_hex_key(TEST_KEY).unwrap();
        let c = s
            .countersign(&cfg(address!("2222222222222222222222222222222222222222")), 8453, 5_000_000, 2_000_000_000)
            .unwrap();
        // cross-checked with `cast call` against the deployed escrow
        assert_eq!(
            c.channel_id,
            "0x79cc65f42d06fc85d9297d4f4c0d656a568451c47745a9a0c53db3d4ee187b19"
        );
        assert_eq!(
            c.digest,
            "0x3ce201974ddfa5e2cd2d082f92e95d6e4edf2c1e0a71973fc93e26a1838a5cf6"
        );
    }
}
