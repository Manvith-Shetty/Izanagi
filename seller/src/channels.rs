//! Per-channel state and the rules for accepting and cashing vouchers.
//!
//! The checks are pure functions so the protocol can be tested without a chain; the server
//! feeds them what it read on chain.

use crate::chain::OnChain;
use crate::env::ClaimConfig;
use crate::screen::Standing;
use alloy::primitives::{Bytes, B256};
use common::x402::{errors, ChannelState, PaymentPayload, PaymentRequirements, VoucherState};
use common::ChannelConfig;
use serde::Serialize;

/// A rejected payment: a spec error code and a sentence for humans.
#[derive(Debug, Clone, PartialEq)]
pub struct Reject {
    pub code: &'static str,
    pub message: String,
}

fn reject(code: &'static str, message: impl Into<String>) -> Reject {
    Reject { code, message: message.into() }
}

/// Verification rules 1-5 of the EVM binding: the payload describes a channel to us, on our
/// terms, and its config hashes to the id it claims.
pub fn check_terms(
    p: &PaymentPayload,
    ours: &PaymentRequirements,
    chain_id: u64,
) -> Result<ChannelConfig, Reject> {
    if p.accepted.scheme != common::x402::SCHEME {
        return Err(reject(errors::SCHEME, format!("scheme {:?} is not batch-settlement", p.accepted.scheme)));
    }
    if p.accepted.network != ours.network {
        return Err(reject(errors::NETWORK_MISMATCH, format!("we settle on {}, not {}", ours.network, p.accepted.network)));
    }
    if p.payload.kind != "voucher" {
        return Err(reject(
            errors::PAYLOAD_TYPE,
            format!(
                "{:?} payloads are not accepted here: open or top up the channel on chain (a Countersign \
                 wallet's owner calls openChannel), then pay with vouchers",
                p.payload.kind
            ),
        ));
    }
    let c = &p.payload.channel_config;
    if c.receiver != ours.pay_to {
        return Err(reject(errors::RECEIVER_MISMATCH, "channel receiver is not our payTo"));
    }
    if c.receiver_authorizer != ours.extra.receiver_authorizer {
        return Err(reject(errors::RECEIVER_AUTHORIZER_MISMATCH, "channel receiverAuthorizer is not ours"));
    }
    if c.token != ours.asset {
        return Err(reject(errors::TOKEN_MISMATCH, "channel token is not the asset we price in"));
    }
    if c.withdraw_delay != ours.extra.withdraw_delay {
        return Err(reject(errors::WITHDRAW_DELAY_MISMATCH, "channel withdrawDelay is not ours"));
    }
    let cfg = c.to_config().map_err(|e| reject(errors::VOUCHER_PAYLOAD, e.to_string()))?;
    if common::channel_id(&cfg, chain_id) != p.payload.voucher.channel_id {
        return Err(reject(errors::CHANNEL_ID_MISMATCH, "channel config does not hash to the claimed channelId"));
    }
    Ok(cfg)
}

/// Rules 7, 9, 10 and the cumulative-amount rule: the voucher is exactly one request more than
/// we have charged, and the escrow holds enough to back it.
pub fn check_amounts(max: u128, charged: u128, price: u128, chain: &OnChain) -> Result<(), Reject> {
    if max != charged + price {
        return Err(reject(
            errors::CUMULATIVE_MISMATCH,
            format!("voucher is for {max}, expected charged {charged} + price {price} = {}", charged + price),
        ));
    }
    if chain.balance == 0 {
        return Err(reject(errors::CHANNEL_NOT_FOUND, "channel has no balance in the escrow"));
    }
    if max > chain.balance {
        return Err(reject(errors::CUMULATIVE_EXCEEDS_BALANCE, format!("voucher for {max} exceeds escrowed {}", chain.balance)));
    }
    if max <= chain.total_claimed {
        return Err(reject(errors::CUMULATIVE_BELOW_CLAIMED, format!("voucher for {max} is not above claimed {}", chain.total_claimed)));
    }
    Ok(())
}

/// What happened the last time we tried to cash a channel in.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum ClaimOutcome {
    Claimed { tx: String, amount: u128, at: u64 },
    /// The escrow refused. For a Countersign payer: revoked, paused, or the attestation lapsed.
    Rejected { reason: String, unclaimable: u128, at: u64 },
    /// We could not reach the chain; nothing is lost yet.
    Failed { error: String, at: u64 },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    #[serde(skip)]
    pub cfg: ChannelConfig,
    pub channel_id: B256,
    pub payer: String,
    /// `chargedCumulativeAmount`: what we have actually earned.
    pub charged: u128,
    /// Latest voucher the payer signed.
    pub signed_max: u128,
    #[serde(skip)]
    pub signature: Bytes,
    /// When the payer is a Countersign wallet: after this, the latest voucher stops being claimable.
    pub attestation_expiry: Option<u64>,
    pub chain: ChainSnapshot,
    pub requests: u64,
    /// When the oldest value we have not yet claimed was earned.
    pub unclaimed_since: Option<u64>,
    pub standing: Standing,
    pub payer_score: f64,
    pub last_claim: Option<ClaimOutcome>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainSnapshot {
    pub balance: u128,
    pub total_claimed: u128,
    pub withdraw_finalize_after: u64,
    pub synced_at: u64,
}

impl Channel {
    /// Cold start: whatever was already claimed on chain is the baseline (spec, server state loss).
    pub fn new(cfg: ChannelConfig, channel_id: B256, chain: &OnChain, now: u64) -> Self {
        Self {
            payer: format!("{:#x}", cfg.payer),
            cfg,
            channel_id,
            charged: chain.total_claimed,
            signed_max: 0,
            signature: Bytes::new(),
            attestation_expiry: None,
            chain: ChainSnapshot::default(),
            requests: 0,
            unclaimed_since: None,
            standing: Standing::Trusted,
            payer_score: 0.0,
            last_claim: None,
        }
        .synced(chain, now)
    }

    pub fn synced(mut self, chain: &OnChain, now: u64) -> Self {
        self.sync(chain, now);
        self
    }

    pub fn sync(&mut self, chain: &OnChain, now: u64) {
        self.chain = ChainSnapshot {
            balance: chain.balance,
            total_claimed: chain.total_claimed,
            withdraw_finalize_after: chain.withdraw_finalize_after,
            synced_at: now,
        };
        if self.unclaimed() == 0 {
            self.unclaimed_since = None;
        }
    }

    pub fn unclaimed(&self) -> u128 {
        self.charged.saturating_sub(self.chain.total_claimed)
    }

    /// Record a served request. Only called after the resource handler succeeded.
    pub fn commit(&mut self, price: u128, max: u128, sig: Bytes, now: u64) {
        if self.unclaimed() == 0 {
            self.unclaimed_since = Some(now);
        }
        self.charged += price;
        self.signed_max = max;
        self.attestation_expiry = common::escrow::decode_countersign_blob(&sig).map(|b| b.expiry);
        self.signature = sig;
        self.requests += 1;
    }

    pub fn channel_state(&self) -> ChannelState {
        ChannelState {
            channel_id: self.channel_id,
            balance: self.chain.balance.to_string(),
            total_claimed: self.chain.total_claimed.to_string(),
            withdraw_requested_at: self.chain.withdraw_finalize_after,
            // this seller does not take refund payloads, so the nonce is not tracked
            refund_nonce: "0".into(),
            charged_cumulative_amount: Some(self.charged.to_string()),
        }
    }

    /// The last voucher we hold, for a corrective 402. `None` before the first one.
    pub fn voucher_state(&self) -> Option<VoucherState> {
        (self.signed_max > 0).then(|| VoucherState {
            signed_max_claimable: self.signed_max.to_string(),
            signature: self.signature.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimReason {
    /// The payer started a timed withdrawal: claim before it drains the escrow.
    WithdrawPending,
    /// The Countersign attestation behind our latest voucher is about to lapse.
    AttestationExpiring,
    /// More unclaimed value than this payer's standing allows us to carry.
    OverCreditLine,
    /// Value left unclaimed longer than we allow, whatever else holds.
    Stale,
    /// An operator asked.
    Manual,
}

/// Should this channel be cashed in now, and why.
pub fn claim_due(ch: &Channel, now: u64, cfg: &ClaimConfig) -> Option<ClaimReason> {
    if ch.unclaimed() == 0 {
        return None;
    }
    if ch.chain.withdraw_finalize_after != 0 {
        return Some(ClaimReason::WithdrawPending);
    }
    if let Some(expiry) = ch.attestation_expiry {
        if now + cfg.margin_secs >= expiry {
            return Some(ClaimReason::AttestationExpiring);
        }
    }
    let credit_line = match ch.standing {
        Standing::Trusted => cfg.max_unclaimed,
        Standing::Careful | Standing::Refused => 0,
    };
    if ch.unclaimed() > credit_line {
        return Some(ClaimReason::OverCreditLine);
    }
    if ch.unclaimed_since.is_some_and(|t| now.saturating_sub(t) >= cfg.max_age_secs) {
        return Some(ClaimReason::Stale);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, Address};
    use common::x402::{ChannelConfigJson, RequirementsExtra, SchemePayload, VoucherJson};

    const PRICE: u128 = 10_000;

    fn ours() -> PaymentRequirements {
        PaymentRequirements {
            scheme: "batch-settlement".into(),
            network: "eip155:8453".into(),
            amount: PRICE.to_string(),
            asset: common::USDC_BASE,
            pay_to: address!("2222222222222222222222222222222222222222"),
            max_timeout_seconds: 3600,
            extra: RequirementsExtra {
                receiver_authorizer: address!("3333333333333333333333333333333333333333"),
                withdraw_delay: 900,
                name: "USD Coin".into(),
                version: "2".into(),
                min_deposit: None,
                channel_state: None,
                voucher_state: None,
            },
        }
    }

    fn payload() -> PaymentPayload {
        let r = ours();
        let cfg = ChannelConfig::countersign(
            address!("1111111111111111111111111111111111111111"),
            r.pay_to,
            r.extra.receiver_authorizer,
            r.asset,
            900,
            B256::ZERO,
        );
        PaymentPayload {
            x402_version: 2,
            accepted: r,
            payload: SchemePayload {
                kind: "voucher".into(),
                channel_config: ChannelConfigJson::from(&cfg),
                voucher: VoucherJson {
                    channel_id: common::channel_id(&cfg, 8453),
                    max_claimable_amount: PRICE.to_string(),
                    signature: Bytes::new(),
                },
                amount: None,
                deposit: None,
            },
        }
    }

    #[test]
    fn accepts_a_voucher_on_our_terms() {
        let cfg = check_terms(&payload(), &ours(), 8453).unwrap();
        assert_eq!(cfg.payerAuthorizer, Address::ZERO, "a Countersign channel routes to EIP-1271");
    }

    #[test]
    fn each_term_mismatch_has_its_own_spec_code() {
        let cases: Vec<(fn(&mut PaymentPayload), &str)> = vec![
            (|p| p.accepted.scheme = "exact".into(), errors::SCHEME),
            (|p| p.accepted.network = "eip155:10".into(), errors::NETWORK_MISMATCH),
            (|p| p.payload.kind = "deposit".into(), errors::PAYLOAD_TYPE),
            (|p| p.payload.channel_config.receiver = Address::ZERO, errors::RECEIVER_MISMATCH),
            (|p| p.payload.channel_config.receiver_authorizer = Address::ZERO, errors::RECEIVER_AUTHORIZER_MISMATCH),
            (|p| p.payload.channel_config.token = Address::ZERO, errors::TOKEN_MISMATCH),
            (|p| p.payload.channel_config.withdraw_delay = 1800, errors::WITHDRAW_DELAY_MISMATCH),
            (|p| p.payload.channel_config.salt = B256::repeat_byte(1), errors::CHANNEL_ID_MISMATCH),
        ];
        for (mutate, code) in cases {
            let mut p = payload();
            mutate(&mut p);
            assert_eq!(check_terms(&p, &ours(), 8453).unwrap_err().code, code);
        }
    }

    #[test]
    fn a_channel_id_is_bound_to_its_chain() {
        // the same payload replayed against an Optimism seller
        let mut r = ours();
        r.network = "eip155:10".into();
        let mut p = payload();
        p.accepted.network = "eip155:10".into();
        assert_eq!(check_terms(&p, &r, 10).unwrap_err().code, errors::CHANNEL_ID_MISMATCH);
    }

    #[test]
    fn amounts_follow_the_cumulative_rules() {
        let chain = OnChain { balance: 1_000_000, total_claimed: 0, withdraw_finalize_after: 0 };
        assert!(check_amounts(PRICE, 0, PRICE, &chain).is_ok());
        assert_eq!(check_amounts(2 * PRICE, 0, PRICE, &chain).unwrap_err().code, errors::CUMULATIVE_MISMATCH);
        assert_eq!(
            check_amounts(PRICE, 0, PRICE, &OnChain { balance: 0, ..chain }).unwrap_err().code,
            errors::CHANNEL_NOT_FOUND
        );
        assert_eq!(
            check_amounts(PRICE, 0, PRICE, &OnChain { balance: PRICE - 1, ..chain }).unwrap_err().code,
            errors::CUMULATIVE_EXCEEDS_BALANCE
        );
        assert_eq!(
            check_amounts(PRICE, 0, PRICE, &OnChain { total_claimed: PRICE, ..chain }).unwrap_err().code,
            errors::CUMULATIVE_BELOW_CLAIMED
        );
    }

    fn channel(now: u64) -> Channel {
        let p = payload();
        let cfg = p.payload.channel_config.to_config().unwrap();
        let chain = OnChain { balance: 1_000_000, total_claimed: 0, withdraw_finalize_after: 0 };
        Channel::new(cfg, p.payload.voucher.channel_id, &chain, now)
    }

    fn claim_cfg() -> ClaimConfig {
        ClaimConfig { tick_secs: 10, margin_secs: 30, max_unclaimed: 50_000, max_age_secs: 3600 }
    }

    #[test]
    fn cold_start_baselines_on_what_was_already_claimed() {
        let chain = OnChain { balance: 1_000_000, total_claimed: 70_000, withdraw_finalize_after: 0 };
        let ch = Channel::new(channel(0).cfg, B256::ZERO, &chain, 0);
        assert_eq!(ch.charged, 70_000);
        assert_eq!(ch.unclaimed(), 0);
    }

    #[test]
    fn nothing_to_claim_before_the_first_request() {
        assert_eq!(claim_due(&channel(0), 0, &claim_cfg()), None);
    }

    #[test]
    fn a_trusted_payer_gets_a_credit_line_a_careful_one_does_not() {
        let mut ch = channel(0);
        ch.commit(PRICE, PRICE, Bytes::from(vec![0u8; 65]), 0);
        assert_eq!(claim_due(&ch, 1, &claim_cfg()), None, "0.01 is inside a 0.05 credit line");

        ch.standing = Standing::Careful;
        assert_eq!(claim_due(&ch, 1, &claim_cfg()), Some(ClaimReason::OverCreditLine));
    }

    #[test]
    fn claims_before_a_countersign_attestation_lapses() {
        let mut ch = channel(0);
        let blob = common::encode_signature_blob(&[1u8; 65], &[2u8; 65], ch.cfg.receiver, 1_000);
        ch.commit(PRICE, PRICE, Bytes::from(blob), 900);
        assert_eq!(ch.attestation_expiry, Some(1_000));
        assert_eq!(claim_due(&ch, 960, &claim_cfg()), None);
        assert_eq!(claim_due(&ch, 970, &claim_cfg()), Some(ClaimReason::AttestationExpiring));
    }

    #[test]
    fn a_pending_withdrawal_is_claimed_first() {
        let mut ch = channel(0);
        ch.commit(PRICE, PRICE, Bytes::from(vec![0u8; 65]), 0);
        ch.sync(&OnChain { balance: 1_000_000, total_claimed: 0, withdraw_finalize_after: 5_000 }, 1);
        assert_eq!(claim_due(&ch, 1, &claim_cfg()), Some(ClaimReason::WithdrawPending));
    }

    #[test]
    fn stale_value_is_claimed_eventually() {
        let mut ch = channel(0);
        ch.commit(PRICE, PRICE, Bytes::from(vec![0u8; 65]), 100);
        assert_eq!(claim_due(&ch, 100 + 3599, &claim_cfg()), None);
        assert_eq!(claim_due(&ch, 100 + 3600, &claim_cfg()), Some(ClaimReason::Stale));
    }

    #[test]
    fn a_landed_claim_clears_the_clock() {
        let mut ch = channel(0);
        ch.commit(PRICE, PRICE, Bytes::from(vec![0u8; 65]), 100);
        ch.sync(&OnChain { balance: 1_000_000, total_claimed: PRICE, withdraw_finalize_after: 0 }, 200);
        assert_eq!(ch.unclaimed(), 0);
        assert_eq!(ch.unclaimed_since, None);
    }
}
