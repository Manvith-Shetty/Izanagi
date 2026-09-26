//! x402 v2 HTTP wire types for the `batch-settlement` scheme on EVM.
//!
//! Field names follow the official spec, `specs/schemes/batch-settlement/scheme_batch_settlement_evm.md`
//! and `specs/transports-v2/http.md` in `x402-foundation/x402`. Every protocol message travels
//! as base64-encoded JSON in one of three headers; response bodies are ours to choose.

use crate::{ChannelConfig, MIN_WITHDRAW_DELAY};
use alloy::primitives::{aliases::U40, Address, Bytes, B256};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Server -> client, on a 402: base64 `PaymentRequired`.
pub const HEADER_REQUIRED: &str = "PAYMENT-REQUIRED";
/// Client -> server: base64 `PaymentPayload`.
pub const HEADER_SIGNATURE: &str = "PAYMENT-SIGNATURE";
/// Server -> client, on a paid response: base64 `SettlementResponse`.
pub const HEADER_RESPONSE: &str = "PAYMENT-RESPONSE";

pub const VERSION: u32 = 2;
pub const SCHEME: &str = "batch-settlement";

/// Spec error codes this implementation can emit.
pub mod errors {
    pub const PAYLOAD_TYPE: &str = "invalid_batch_settlement_evm_payload_type";
    pub const VOUCHER_PAYLOAD: &str = "invalid_batch_settlement_evm_voucher_payload";
    pub const SCHEME: &str = "invalid_batch_settlement_evm_scheme";
    pub const NETWORK_MISMATCH: &str = "invalid_batch_settlement_evm_network_mismatch";
    pub const CHANNEL_ID_MISMATCH: &str = "invalid_batch_settlement_evm_channel_id_mismatch";
    pub const RECEIVER_MISMATCH: &str = "invalid_batch_settlement_evm_receiver_mismatch";
    pub const RECEIVER_AUTHORIZER_MISMATCH: &str =
        "invalid_batch_settlement_evm_receiver_authorizer_mismatch";
    pub const TOKEN_MISMATCH: &str = "invalid_batch_settlement_evm_token_mismatch";
    pub const WITHDRAW_DELAY_MISMATCH: &str = "invalid_batch_settlement_evm_withdraw_delay_mismatch";
    pub const CUMULATIVE_MISMATCH: &str = "invalid_batch_settlement_evm_cumulative_amount_mismatch";
    pub const CUMULATIVE_BELOW_CLAIMED: &str = "invalid_batch_settlement_evm_cumulative_below_claimed";
    pub const CUMULATIVE_EXCEEDS_BALANCE: &str =
        "invalid_batch_settlement_evm_cumulative_exceeds_balance";
    pub const CHANNEL_NOT_FOUND: &str = "invalid_batch_settlement_evm_channel_not_found";
    pub const VOUCHER_SIGNATURE: &str = "invalid_batch_settlement_evm_voucher_signature";
    pub const RPC_READ_FAILED: &str = "invalid_batch_settlement_evm_rpc_read_failed";
}

/// CAIP-2 network id for an EVM chain.
pub fn network(chain_id: u64) -> String {
    format!("eip155:{chain_id}")
}

pub fn encode_header<T: Serialize>(value: &T) -> Result<String> {
    Ok(STANDARD.encode(serde_json::to_vec(value)?))
}

pub fn decode_header<T: DeserializeOwned>(header: &str) -> Result<T> {
    let raw = STANDARD.decode(header.trim()).context("header is not base64")?;
    serde_json::from_slice(&raw).context("header is not the expected JSON")
}

/// Atomic token amounts travel as decimal strings.
pub fn parse_amount(s: &str) -> Result<u128> {
    s.parse().map_err(|_| anyhow!("amount {s:?} is not a non-negative integer"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub url: String,
    pub description: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequired {
    pub x402_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<Resource>,
    pub accepts: Vec<PaymentRequirements>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirements {
    pub scheme: String,
    pub network: String,
    /// Maximum per-request price, atomic units.
    pub amount: String,
    pub asset: Address,
    pub pay_to: Address,
    pub max_timeout_seconds: u64,
    pub extra: RequirementsExtra,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequirementsExtra {
    pub receiver_authorizer: Address,
    pub withdraw_delay: u64,
    /// EIP-712 domain of the token contract.
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_deposit: Option<String>,
    /// Corrective 402 only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_state: Option<ChannelState>,
    /// Corrective 402 only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voucher_state: Option<VoucherState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelState {
    pub channel_id: B256,
    pub balance: String,
    pub total_claimed: String,
    pub withdraw_requested_at: u64,
    pub refund_nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charged_cumulative_amount: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoucherState {
    pub signed_max_claimable: String,
    pub signature: Bytes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayload {
    pub x402_version: u32,
    pub accepted: PaymentRequirements,
    pub payload: SchemePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemePayload {
    /// `deposit` | `voucher` | `refund`
    #[serde(rename = "type")]
    pub kind: String,
    pub channel_config: ChannelConfigJson,
    pub voucher: VoucherJson,
    /// Refund only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    /// Deposit only. Kept opaque: this implementation does not accept deposits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deposit: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelConfigJson {
    pub payer: Address,
    pub payer_authorizer: Address,
    pub receiver: Address,
    pub receiver_authorizer: Address,
    pub token: Address,
    pub withdraw_delay: u64,
    pub salt: B256,
}

impl From<&ChannelConfig> for ChannelConfigJson {
    fn from(c: &ChannelConfig) -> Self {
        Self {
            payer: c.payer,
            payer_authorizer: c.payerAuthorizer,
            receiver: c.receiver,
            receiver_authorizer: c.receiverAuthorizer,
            token: c.token,
            withdraw_delay: c.withdrawDelay.to::<u64>(),
            salt: c.salt,
        }
    }
}

impl ChannelConfigJson {
    pub fn to_config(&self) -> Result<ChannelConfig> {
        // the escrow's bounds: 15 minutes to 30 days
        if !(MIN_WITHDRAW_DELAY..=30 * 24 * 3600).contains(&self.withdraw_delay) {
            return Err(anyhow!("withdrawDelay {} outside 15 min - 30 days", self.withdraw_delay));
        }
        Ok(ChannelConfig {
            payer: self.payer,
            payerAuthorizer: self.payer_authorizer,
            receiver: self.receiver,
            receiverAuthorizer: self.receiver_authorizer,
            token: self.token,
            withdrawDelay: U40::from(self.withdraw_delay),
            salt: self.salt,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoucherJson {
    pub channel_id: B256,
    /// Cumulative ceiling, atomic units.
    pub max_claimable_amount: String,
    pub signature: Bytes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementResponse {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    /// Empty for a voucher: nothing moved on chain.
    pub transaction: String,
    pub network: String,
    pub payer: String,
    /// Empty for a voucher: nothing was transferred.
    #[serde(default)]
    pub amount: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<SettlementExtra>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementExtra {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charged_amount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_state: Option<ChannelState>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// The voucher payload from the spec, verbatim apart from concrete addresses.
    #[test]
    fn decodes_the_spec_voucher_payload() {
        let json = r#"{
          "x402Version": 2,
          "accepted": {
            "scheme": "batch-settlement",
            "network": "eip155:8453",
            "amount": "1000",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "payTo": "0x2222222222222222222222222222222222222222",
            "maxTimeoutSeconds": 3600,
            "extra": {
              "receiverAuthorizer": "0x3333333333333333333333333333333333333333",
              "withdrawDelay": 900,
              "name": "USDC",
              "version": "2"
            }
          },
          "payload": {
            "type": "voucher",
            "channelConfig": {
              "payer": "0x1111111111111111111111111111111111111111",
              "payerAuthorizer": "0x0000000000000000000000000000000000000000",
              "receiver": "0x2222222222222222222222222222222222222222",
              "receiverAuthorizer": "0x3333333333333333333333333333333333333333",
              "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
              "withdrawDelay": 900,
              "salt": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "voucher": {
              "channelId": "0x79cc65f42d06fc85d9297d4f4c0d656a568451c47745a9a0c53db3d4ee187b19",
              "maxClaimableAmount": "5000",
              "signature": "0xabcd"
            }
          }
        }"#;
        let p: PaymentPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.payload.kind, "voucher");
        assert_eq!(p.accepted.pay_to, address!("2222222222222222222222222222222222222222"));
        assert_eq!(parse_amount(&p.payload.voucher.max_claimable_amount).unwrap(), 5000);

        // the config round-trips to the same channel id the live escrow computes
        let cfg = p.payload.channel_config.to_config().unwrap();
        assert_eq!(crate::channel_id(&cfg, 8453), p.payload.voucher.channel_id);
        assert_eq!(ChannelConfigJson::from(&cfg), p.payload.channel_config);
    }

    #[test]
    fn headers_round_trip_through_base64() {
        let r = SettlementResponse {
            success: true,
            error_reason: None,
            transaction: String::new(),
            network: network(8453),
            payer: "0x1".into(),
            amount: String::new(),
            extra: Some(SettlementExtra { charged_amount: Some("700".into()), channel_state: None }),
        };
        let back: SettlementResponse = decode_header(&encode_header(&r).unwrap()).unwrap();
        assert_eq!(back.extra.unwrap().charged_amount.as_deref(), Some("700"));
        assert!(decode_header::<SettlementResponse>("not base64!").is_err());
    }

    #[test]
    fn rejects_withdraw_delay_outside_the_escrow_bounds() {
        let mut c = ChannelConfigJson {
            payer: Address::ZERO,
            payer_authorizer: Address::ZERO,
            receiver: Address::ZERO,
            receiver_authorizer: Address::ZERO,
            token: Address::ZERO,
            withdraw_delay: 60,
            salt: B256::ZERO,
        };
        assert!(c.to_config().is_err());
        c.withdraw_delay = 900;
        assert!(c.to_config().is_ok());
    }

    #[test]
    fn amounts_must_be_integers() {
        assert_eq!(parse_amount("10000").unwrap(), 10_000);
        assert!(parse_amount("-1").is_err());
        assert!(parse_amount("0.01").is_err());
    }
}
