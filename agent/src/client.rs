//! The agent-side client.
//!
//! This is deliberately thin. It holds the agent's key and **nothing else**: no Intercepta
//! key, no OIDC client secret, no countersigning key. It asks the countersigner for
//! permission and assembles the two-of-two blob. It cannot decide anything.
//!
//! That is the point. Rip this library out, sign vouchers by hand, and the escrow still
//! rejects them — because the enforcement lives in the contract, not here.

use alloy::primitives::{Address, B256};
use alloy::signers::{local::PrivateKeySigner, SignerSync};
use anyhow::{anyhow, Result};
use common::{channel_id, encode_signature_blob, voucher_digest, ChannelConfig};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Countersignature {
    pub oracle_signature: String,
    pub digest: String,
    pub channel_id: String,
    pub ceiling: u128,
    pub seller: String,
    pub expiry: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    /// Opaque handle. The OAuth `device_code` stays in the countersigner, where it belongs.
    pub approval_id: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    #[serde(default = "default_expires")]
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// What the human is being asked to approve.
    #[serde(default)]
    pub seller: String,
    /// The limit the human is asked to approve for this tab.
    #[serde(default)]
    pub ceiling: u128,
    /// The cumulative amount the agent actually asked for.
    #[serde(default)]
    pub requested: u128,
    /// What a human had already approved for this tab.
    #[serde(default)]
    pub approved_so_far: u128,
    /// A page that shows the human exactly what they are approving.
    #[serde(default)]
    pub approval_url: Option<String>,
}

fn default_expires() -> u64 { 600 }
fn default_interval() -> u64 { 5 }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountersignResponse {
    pub verdict: serde_json::Value,
    pub reason: String,
    pub toxic_score: f64,
    #[serde(default)]
    pub countersignature: Option<Countersignature>,
    #[serde(default)]
    pub approval: Option<PendingApproval>,
    /// What the screening saw.
    #[serde(default)]
    pub evidence: Option<serde_json::Value>,
}

/// Every way a request for a voucher can end.
#[derive(Debug, Clone)]
pub enum Authorization {
    /// A claimable voucher, co-signed by the agent.
    Signed(CountersignResponse, SignedVoucher),
    /// A person has to approve first; `response.approval` says how.
    NeedsHuman(CountersignResponse),
    /// No voucher exists, and the response says why.
    Refused(CountersignResponse),
}

impl CountersignResponse {
    pub fn action(&self) -> &str {
        self.verdict.get("action").and_then(|v| v.as_str()).unwrap_or("unknown")
    }
}

/// What the agent ends up with: a voucher the seller can actually claim.
#[derive(Debug, Clone)]
pub struct SignedVoucher {
    pub channel_id: B256,
    pub digest: B256,
    pub ceiling: u128,
    /// After this unix time the attestation lapses and the voucher is no longer claimable.
    pub expiry: u64,
    /// `abi.encode(agentSig, oracleSig, seller, expiry)` — goes in the voucher's
    /// `signature` field, and is handed to `isValidSignature` at claim time.
    pub signature_blob: Vec<u8>,
}

pub struct CountersignClient {
    http: reqwest::Client,
    base_url: String,
    agent: PrivateKeySigner,
}

impl CountersignClient {
    pub fn new(base_url: impl Into<String>, agent_key: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            agent: agent_key.trim().trim_start_matches("0x").parse()?,
        })
    }

    pub fn address(&self) -> Address {
        self.agent.address()
    }

    /// Ask the countersigner to approve a cumulative ceiling, then co-sign it.
    ///
    /// Returns `Ok(None)` when a human must approve first — the caller is handed the device
    /// challenge to display. Returns `Err` when the payment was refused outright.
    pub async fn authorize(
        &self,
        cfg: &ChannelConfig,
        chain_id: u64,
        ceiling: u128,
        approval_id: Option<&str>,
    ) -> Result<(CountersignResponse, Option<SignedVoucher>)> {
        match self.request(cfg, chain_id, ceiling, approval_id).await? {
            Authorization::Signed(r, v) => Ok((r, Some(v))),
            Authorization::NeedsHuman(r) => Ok((r, None)),
            Authorization::Refused(r) => Err(anyhow!("refused: {}", r.reason)),
        }
    }

    /// Like `authorize`, but a refusal keeps its reason, score and evidence.
    pub async fn request(
        &self,
        cfg: &ChannelConfig,
        chain_id: u64,
        ceiling: u128,
        approval_id: Option<&str>,
    ) -> Result<Authorization> {
        let body = serde_json::json!({
            "channel": {
                "payer": format!("{:#x}", cfg.payer),
                "receiver": format!("{:#x}", cfg.receiver),
                "receiverAuthorizer": format!("{:#x}", cfg.receiverAuthorizer),
                "token": format!("{:#x}", cfg.token),
                "withdrawDelay": cfg.withdrawDelay.to::<u64>(),
                "salt": format!("{:#x}", cfg.salt),
            },
            "chainId": chain_id,
            "ceiling": ceiling,
            "approvalId": approval_id,
        });

        let res = self
            .http
            .post(format!("{}/v1/countersign", self.base_url))
            .json(&body)
            .send()
            .await?;
        let status = res.status();
        let parsed: CountersignResponse = res.json().await?;

        if parsed.countersignature.is_none() {
            // `ask` carries an approval; anything else without a signature is a refusal
            return Ok(if status.as_u16() == 202 && parsed.approval.is_some() {
                Authorization::NeedsHuman(parsed)
            } else {
                Authorization::Refused(parsed)
            });
        }

        let cs = parsed.countersignature.clone().unwrap();
        let cid = channel_id(cfg, chain_id);
        let digest = voucher_digest(cid, cs.ceiling, chain_id);

        // sanity: the countersigner must have signed the voucher we think we are making
        let claimed: B256 = cs.digest.parse()?;
        if claimed != digest {
            return Err(anyhow!(
                "countersigner signed a different digest ({claimed:#x} != {digest:#x})"
            ));
        }

        let agent_sig = self.agent.sign_hash_sync(&digest)?;
        let oracle_sig = parse_hex(&cs.oracle_signature)?;
        let blob = encode_signature_blob(
            &agent_sig.as_bytes(),
            &oracle_sig,
            cfg.receiver,
            cs.expiry,
        );

        let voucher = SignedVoucher {
            channel_id: cid,
            digest,
            ceiling: cs.ceiling,
            expiry: cs.expiry,
            signature_blob: blob,
        };
        Ok(Authorization::Signed(parsed, voucher))
    }

    /// Wait for the human, honouring the IdP's polling interval and backing off on
    /// `slow_down`. Returns the pairwise `sub` on approval.
    ///
    /// The agent never sees an id_token and never decides anything: the countersigner
    /// verifies the signature against World's JWKS and hands back only the subject.
    pub async fn await_approval(
        &self,
        approval_id: &str,
        interval_secs: u64,
        expires_in: u64,
        mut on_tick: impl FnMut(u64),
    ) -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);
        let mut interval = interval_secs.max(1);
        let mut waited = 0u64;
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(anyhow!("device code expired before the human approved"));
            }
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            waited += interval;
            on_tick(waited);
            match self.poll_approval(approval_id).await? {
                ApprovalStatus::Approved => return Ok(()),
                ApprovalStatus::SlowDown => interval += 5,
                ApprovalStatus::Pending => {}
            }
        }
    }

    /// Poll once. The countersigner validates; we just relay the state.
    pub async fn poll_approval(&self, approval_id: &str) -> Result<ApprovalStatus> {
        let res = self
            .http
            .post(format!("{}/v1/approve/poll", self.base_url))
            .json(&serde_json::json!({ "approvalId": approval_id }))
            .send()
            .await?;
        let status = res.status();
        let v: serde_json::Value = res.json().await.unwrap_or(serde_json::Value::Null);
        match status.as_u16() {
            // the subject deliberately never reaches the agent
            200 => Ok(ApprovalStatus::Approved),
            202 if v.get("slowDown").and_then(|b| b.as_bool()).unwrap_or(false) => {
                Ok(ApprovalStatus::SlowDown)
            }
            202 => Ok(ApprovalStatus::Pending),
            // denied / expired / cancelled / stale / insufficient assurance
            _ => Err(anyhow!(
                "{}",
                v.get("reason").and_then(|r| r.as_str()).unwrap_or("denied")
            )),
        }
    }
}

/// Where a human approval has got to.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalStatus {
    Pending,
    /// The IdP asked us to poll less often.
    SlowDown,
    /// The human approved. The subject stays in the countersigner.
    Approved,
}

pub fn parse_hex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim().trim_start_matches("0x");
    if s.len() % 2 != 0 {
        return Err(anyhow!("odd-length hex"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow!("{e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent and the countersigner are separate crates with separate structs for the same
    /// JSON. This decodes the countersigner's REAL serialised output, so the two cannot drift
    /// apart again (they once did: snake_case on one side, camelCase on the other).
    #[test]
    fn decodes_what_the_countersigner_actually_sends() {
        use alloy::primitives::{address, B256};
        let oracle = countersigner_lib::signer::OracleSigner::from_hex_key(
            "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
        )
        .unwrap();
        let cfg = ChannelConfig::countersign(
            address!("1111111111111111111111111111111111111111"),
            address!("2222222222222222222222222222222222222222"),
            address!("3333333333333333333333333333333333333333"),
            common::USDC_BASE,
            900,
            B256::ZERO,
        );
        let sig = oracle.countersign(&cfg, 8453, 10_000, 2_000_000_000).unwrap();
        let wire = serde_json::json!({
            "verdict": {"action": "pay", "ceiling": 10000},
            "reason": "no adverse signals",
            "toxicScore": 0.0,
            "countersignature": sig,
        });

        let r: CountersignResponse = serde_json::from_value(wire).expect("the agent must decode the countersigner's response");
        let cs = r.countersignature.expect("countersignature present");
        assert_eq!(cs.ceiling, 10_000);
        assert_eq!(cs.expiry, 2_000_000_000);
        assert_eq!(cs.channel_id, format!("{:#x}", channel_id(&cfg, 8453)));
        assert_eq!(cs.digest, format!("{:#x}", voucher_digest(channel_id(&cfg, 8453), 10_000, 8453)));
        assert_eq!(parse_hex(&cs.oracle_signature).unwrap().len(), 65);
    }

    #[test]
    fn parses_hex_signatures() {
        assert_eq!(parse_hex("0x0aff").unwrap(), vec![0x0a, 0xff]);
        assert!(parse_hex("0xabc").is_err());
    }

    #[test]
    fn client_exposes_the_agent_address() {
        let c = CountersignClient::new(
            "http://localhost:8787",
            "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
        )
        .unwrap();
        assert_eq!(
            format!("{:#x}", c.address()),
            "0x9965507d1a55bcc2695c58ba16fb37d819b0a4dc"
        );
    }
}
