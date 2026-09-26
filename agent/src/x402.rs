//! The agent's side of x402 `batch-settlement`: read a seller's terms, build the channel they
//! imply, and pay each request with a countersigned cumulative voucher.
//!
//! Nothing here can authorise a payment. The voucher it sends is only claimable because the
//! countersigner attested it -- and only for as long as the payer's wallet keeps agreeing.

use crate::client::SignedVoucher;
use alloy::primitives::{Address, Bytes, B256};
use anyhow::{anyhow, Context, Result};
use common::x402::{
    self, decode_header, encode_header, parse_amount, ChannelConfigJson, PaymentRequirements,
    SchemePayload, SettlementResponse, VoucherJson,
};
use common::ChannelConfig;

/// A seller's `batch-settlement` offer: typed for us to act on, raw for echoing back.
#[derive(Debug, Clone)]
pub struct Offer {
    pub terms: PaymentRequirements,
    /// Exactly what the seller sent. The payment's `accepted` must echo the chosen
    /// requirement, and a round trip through our struct would drop fields we do not model.
    pub raw: serde_json::Value,
}

/// Pick the offer we can pay from a 402's `PAYMENT-REQUIRED` header.
///
/// Real sellers list several schemes side by side (`exact` on two networks, then
/// `batch-settlement`), and only ours has to fit our types. So the list is read loosely and
/// only the one offer we will pay is decoded strictly.
pub fn terms_from_402(header: &str, chain_id: u64) -> Result<Offer> {
    let pr: serde_json::Value = decode_header(header).context("decoding PAYMENT-REQUIRED")?;
    let network = x402::network(chain_id);
    let offered: Vec<String> = pr["accepts"]
        .as_array()
        .map(|a| a.iter().map(|o| format!("{} on {}", o["scheme"].as_str().unwrap_or("?"), o["network"].as_str().unwrap_or("?"))).collect())
        .unwrap_or_default();
    let raw = pr["accepts"]
        .as_array()
        .and_then(|a| a.iter().find(|o| o["scheme"] == x402::SCHEME && o["network"] == network.as_str()))
        .cloned()
        .ok_or_else(|| anyhow!("seller offers no {} terms on {network} (it offers: {})", x402::SCHEME, offered.join(", ")))?;
    let terms: PaymentRequirements =
        serde_json::from_value(raw.clone()).context("the seller's batch-settlement offer is malformed")?;
    Ok(Offer { terms, raw })
}

/// The seller fixes receiver, authorizer, token and delay; we add ourselves as payer.
/// `payerAuthorizer` is zero, so every voucher is checked by our Countersign wallet.
pub fn channel_for(terms: &PaymentRequirements, wallet: Address, salt: B256) -> ChannelConfig {
    ChannelConfig::countersign(
        wallet,
        terms.pay_to,
        terms.extra.receiver_authorizer,
        terms.asset,
        terms.extra.withdraw_delay,
        salt,
    )
}

/// The `PAYMENT-SIGNATURE` header for one voucher, echoing the seller's offer verbatim.
pub fn payment_header(offer: &Offer, cfg: &ChannelConfig, v: &SignedVoucher) -> Result<String> {
    let payload = SchemePayload {
        kind: "voucher".into(),
        channel_config: ChannelConfigJson::from(cfg),
        voucher: VoucherJson {
            channel_id: v.channel_id,
            max_claimable_amount: v.ceiling.to_string(),
            signature: Bytes::from(v.signature_blob.clone()),
        },
        amount: None,
        deposit: None,
    };
    encode_header(&serde_json::json!({
        "x402Version": x402::VERSION,
        "accepted": offer.raw,
        "payload": payload,
    }))
}

/// What the seller says it charged, applied by the spec's client rules: the response is
/// untrusted, so a charge above the advertised price is refused rather than adopted.
pub fn charged_from_response(header: Option<&str>, price: u128) -> Result<u128> {
    let Some(h) = header else { return Ok(0) };
    let r: SettlementResponse = decode_header(h).context("decoding PAYMENT-RESPONSE")?;
    let charged = match r.extra.and_then(|e| e.charged_amount) {
        Some(c) => parse_amount(&c)?,
        None => 0,
    };
    if charged > price {
        return Err(anyhow!("seller reports charging {charged}, above its own price {price}"));
    }
    Ok(charged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use common::x402::{PaymentRequired, RequirementsExtra, SettlementExtra};

    fn terms() -> PaymentRequirements {
        PaymentRequirements {
            scheme: "batch-settlement".into(),
            network: "eip155:8453".into(),
            amount: "10000".into(),
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

    #[test]
    fn picks_batch_settlement_terms_on_our_chain() {
        let mut exact = terms();
        exact.scheme = "exact".into();
        let mut optimism = terms();
        optimism.network = "eip155:10".into();
        let pr = PaymentRequired { x402_version: 2, error: None, resource: None, accepts: vec![exact, optimism, terms()] };
        let h = encode_header(&pr).unwrap();
        assert_eq!(terms_from_402(&h, 8453).unwrap().terms, terms());
        assert!(terms_from_402(&h, 42161).is_err());
    }

    /// Real 402s captured from live Bazaar sellers on 2026-09-26. Their `exact` offers do not
    /// have our fields, which used to fail the whole decode before any payment was tried.
    #[test]
    fn reads_the_batch_settlement_offer_from_real_sellers() {
        for (name, header, pay_to, delay) in [
            ("hyperextend", include_str!("../fixtures/402-hyperextend.b64"), "0x548fC289526ab2F0391D562a723cdA64Bbf1abc4", 86400),
            ("onesource", include_str!("../fixtures/402-onesource.b64"), "0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea", 86400),
        ] {
            let offer = terms_from_402(header.trim(), 8453).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(offer.terms.scheme, "batch-settlement", "{name}");
            assert_eq!(offer.terms.pay_to, pay_to.parse::<Address>().unwrap(), "{name}");
            assert_eq!(offer.terms.extra.withdraw_delay, delay, "{name}");
            assert_eq!(offer.raw["scheme"], "batch-settlement");
        }
        // and a chain they do not settle on is refused with what they do offer
        let e = terms_from_402(include_str!("../fixtures/402-onesource.b64").trim(), 10).unwrap_err();
        assert!(format!("{e:#}").contains("exact on eip155:8453"), "{e:#}");
    }

    #[test]
    fn the_payment_echoes_the_sellers_offer_verbatim() {
        let offer = terms_from_402(include_str!("../fixtures/402-onesource.b64").trim(), 8453).unwrap();
        let cfg = channel_for(&offer.terms, address!("1111111111111111111111111111111111111111"), B256::ZERO);
        let v = SignedVoucher {
            channel_id: common::channel_id(&cfg, 8453),
            digest: B256::ZERO,
            ceiling: 1000,
            expiry: 0,
            signature_blob: vec![1, 2, 3],
        };
        let sent: serde_json::Value = decode_header(&payment_header(&offer, &cfg, &v).unwrap()).unwrap();
        // onesource puts fields in its offer that we do not model; they must survive the echo
        assert_eq!(sent["accepted"], offer.raw);
        assert!(sent["accepted"].get("maxAmountRequired").is_some());
        assert_eq!(sent["payload"]["type"], "voucher");
        assert_eq!(sent["payload"]["voucher"]["maxClaimableAmount"], "1000");
    }

    #[test]
    fn the_channel_routes_vouchers_through_our_wallet() {
        let wallet = address!("1111111111111111111111111111111111111111");
        let cfg = channel_for(&terms(), wallet, B256::ZERO);
        assert_eq!(cfg.payer, wallet);
        assert_eq!(cfg.payerAuthorizer, Address::ZERO);
        assert_eq!(cfg.receiver, terms().pay_to);
    }

    #[test]
    fn an_overcharge_is_not_adopted() {
        let r = |c: &str| {
            encode_header(&SettlementResponse {
                success: true,
                error_reason: None,
                transaction: String::new(),
                network: "eip155:8453".into(),
                payer: "0x1".into(),
                amount: String::new(),
                extra: Some(SettlementExtra { charged_amount: Some(c.into()), channel_state: None }),
            })
            .unwrap()
        };
        assert_eq!(charged_from_response(Some(&r("7000")), 10_000).unwrap(), 7_000);
        assert!(charged_from_response(Some(&r("20000")), 10_000).is_err());
        assert_eq!(charged_from_response(None, 10_000).unwrap(), 0, "missing is zero (spec)");
    }
}
