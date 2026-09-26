//! Shared x402 `batch-settlement` types and Countersign's two-of-two signature format.
//!
//! Everything here is byte-compatible with Coinbase's deployed `x402BatchSettlement`
//! escrow at `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003` (Base / Optimism / Arbitrum).

pub mod escrow;
pub mod intercepta;
pub mod utils;
pub mod x402;

use alloy::dyn_abi::Eip712Domain;
use alloy::primitives::{address, aliases::U40, keccak256, Address, B256, U256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct, SolValue};

/// Coinbase's x402 batch-settlement escrow. Same address on every supported chain (CREATE2).
pub const ESCROW: Address = address!("4020074e9dF2ce1deE5A9C1b5c3f541D02a10003");

pub const USDC_BASE: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
pub const USDC_OPTIMISM: Address = address!("0b2C639c533813f4Aa9D7837CAf62653d097Ff85");
pub const USDC_ARBITRUM: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");

sol! {
    /// Immutable channel identity. `payerAuthorizer == address(0)` is what routes voucher
    /// validation to EIP-1271 on `payer` -- i.e. to the Countersign contract.
    #[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct ChannelConfig {
        address payer;
        address payerAuthorizer;
        address receiver;
        address receiverAuthorizer;
        address token;
        uint40  withdrawDelay;
        bytes32 salt;
    }

    /// EIP-712 payload the payer signs. Note this is the *digest* form used by
    /// `getVoucherDigest`, not the nested Solidity struct used in calldata.
    #[derive(Debug, PartialEq, Eq)]
    struct Voucher {
        bytes32 channelId;
        uint128 maxClaimableAmount;
    }
}

/// Minimum `withdrawDelay` the escrow accepts (15 minutes).
pub const MIN_WITHDRAW_DELAY: u64 = 900;

impl ChannelConfig {
    /// Build a Countersign-style channel: `payerAuthorizer` is always zero, which is what
    /// makes the escrow validate every voucher through EIP-1271 on the Countersign contract.
    pub fn countersign(
        countersign_wallet: Address,
        receiver: Address,
        receiver_authorizer: Address,
        token: Address,
        withdraw_delay: u64,
        salt: B256,
    ) -> Self {
        Self {
            payer: countersign_wallet,
            payerAuthorizer: Address::ZERO,
            receiver,
            receiverAuthorizer: receiver_authorizer,
            token,
            withdrawDelay: U40::from(withdraw_delay),
            salt,
        }
    }
}

/// The EIP-712 domain of the deployed escrow, read from `eip712Domain()` on chain:
/// name `"x402 Batch Settlement"`, version `"1"`, the chain id, the escrow address.
pub fn domain(chain_id: u64) -> Eip712Domain {
    eip712_domain! {
        name: "x402 Batch Settlement",
        version: "1",
        chain_id: chain_id,
        verifying_contract: ESCROW,
    }
}

/// `channelId = _hashTypedDataV4(keccak256(abi.encode(CHANNEL_CONFIG_TYPEHASH, config)))`
pub fn channel_id(cfg: &ChannelConfig, chain_id: u64) -> B256 {
    cfg.eip712_signing_hash(&domain(chain_id))
}

/// `voucherDigest = _hashTypedDataV4(keccak256(abi.encode(VOUCHER_TYPEHASH, channelId, amount)))`
pub fn voucher_digest(channel_id: B256, max_claimable: u128, chain_id: u64) -> B256 {
    Voucher { channelId: channel_id, maxClaimableAmount: max_claimable }
        .eip712_signing_hash(&domain(chain_id))
}

/// The voucher as `eth_signTypedData_v4` JSON: exactly what `voucher_digest` hashes, in the
/// form a wallet (or Intercepta's signature analysis) reads. Amounts are decimal strings so
/// a `uint128` survives JSON.
pub fn voucher_typed_data(channel_id: B256, max_claimable: u128, chain_id: u64) -> serde_json::Value {
    serde_json::json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" },
            ],
            "Voucher": [
                { "name": "channelId", "type": "bytes32" },
                { "name": "maxClaimableAmount", "type": "uint128" },
            ],
        },
        "primaryType": "Voucher",
        "domain": {
            "name": "x402 Batch Settlement",
            "version": "1",
            "chainId": chain_id,
            "verifyingContract": format!("{ESCROW:#x}"),
        },
        "message": {
            "channelId": format!("{channel_id:#x}"),
            "maxClaimableAmount": max_claimable.to_string(),
        },
    })
}

/// The hash the risk oracle countersigns.
///
/// Deliberately a bare struct hash rather than full EIP-712: it is never shown to a wallet,
/// only recovered inside `Countersign.isValidSignature`. It binds the verdict to one exact
/// voucher digest, one seller, and one expiry -- so an attestation cannot be moved to a
/// different channel, a different amount, or a different counterparty.
pub fn attestation_hash(digest: B256, seller: Address, expiry: u64) -> B256 {
    keccak256(
        (
            attestation_typehash(),
            digest,
            seller,
            U256::from(expiry),
        )
            .abi_encode(),
    )
}

pub fn attestation_typehash() -> B256 {
    keccak256(b"Attestation(bytes32 digest,address seller,uint64 expiry)")
}

/// Countersign's signature blob, placed in the voucher's `signature` field.
///
/// `abi.encode(bytes agentSig, bytes oracleSig, address seller, uint64 expiry)`
pub fn encode_signature_blob(
    agent_sig: &[u8],
    oracle_sig: &[u8],
    seller: Address,
    expiry: u64,
) -> Vec<u8> {
    (
        agent_sig.to_vec(),
        oracle_sig.to_vec(),
        seller,
        U256::from(expiry),
    )
        .abi_encode_params()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::b256;

    /// What Intercepta screens must be what the agent signs: the typed data re-hashes to
    /// the voucher digest.
    #[test]
    fn voucher_typed_data_hashes_to_the_signed_digest() {
        let cid = b256!("79cc65f4000000000000000000000000000000000000000000000000000000aa");
        let ceiling = u128::MAX - 1;
        let typed: alloy::dyn_abi::TypedData =
            serde_json::from_value(voucher_typed_data(cid, ceiling, 8453)).unwrap();
        assert_eq!(typed.eip712_signing_hash().unwrap(), voucher_digest(cid, ceiling, 8453));
    }

    /// Cross-checked against `cast call` on Base mainnet -- see README.
    #[test]
    fn typehashes_match_deployed_contract() {
        assert_eq!(
            keccak256(b"ChannelConfig(address payer,address payerAuthorizer,address receiver,address receiverAuthorizer,address token,uint40 withdrawDelay,bytes32 salt)"),
            b256!("1c9a06ceab9b0ebbd3301dc56c9111bb6d9af421356dc9ccb3b7084c755db308"),
            "CHANNEL_CONFIG_TYPEHASH"
        );
        assert_eq!(
            keccak256(b"Voucher(bytes32 channelId,uint128 maxClaimableAmount)"),
            b256!("1e1bd6ff84c3e0d9029a292b212e039c0ca97ec497c55191a4a5874294609a69"),
            "VOUCHER_TYPEHASH"
        );
    }

    #[test]
    fn alloy_derives_the_same_typehashes() {
        assert_eq!(
            ChannelConfig::eip712_type_hash(&ChannelConfig {
                payer: Address::ZERO,
                payerAuthorizer: Address::ZERO,
                receiver: Address::ZERO,
                receiverAuthorizer: Address::ZERO,
                token: Address::ZERO,
                withdrawDelay: U40::ZERO,
                salt: B256::ZERO,
            }),
            b256!("1c9a06ceab9b0ebbd3301dc56c9111bb6d9af421356dc9ccb3b7084c755db308")
        );
        assert_eq!(
            Voucher { channelId: B256::ZERO, maxClaimableAmount: 0 }
                .eip712_type_hash(),
            b256!("1e1bd6ff84c3e0d9029a292b212e039c0ca97ec497c55191a4a5874294609a69")
        );
    }
}

#[cfg(test)]
mod chain_parity {
    use super::*;
    use alloy::primitives::{address, b256};

    /// Reference values pulled from the LIVE contract on Base mainnet via `cast call`:
    ///   getChannelId(cfg)                 -> 0x79cc65f4...
    ///   getVoucherDigest(channelId, 5e6)  -> 0x3ce20197...
    /// If this test passes, our off-chain signing is byte-identical to the escrow's.
    #[test]
    fn digests_match_live_base_mainnet_contract() {
        let cfg = ChannelConfig::countersign(
            address!("1111111111111111111111111111111111111111"),
            address!("2222222222222222222222222222222222222222"),
            address!("3333333333333333333333333333333333333333"),
            USDC_BASE,
            900,
            b256!("0000000000000000000000000000000000000000000000000000000000000001"),
        );
        let cid = channel_id(&cfg, 8453);
        assert_eq!(
            cid,
            b256!("79cc65f42d06fc85d9297d4f4c0d656a568451c47745a9a0c53db3d4ee187b19"),
            "channelId must match the deployed escrow"
        );
        assert_eq!(
            voucher_digest(cid, 5_000_000, 8453),
            b256!("3ce201974ddfa5e2cd2d082f92e95d6e4edf2c1e0a71973fc93e26a1838a5cf6"),
            "voucher digest must match the deployed escrow"
        );
    }
}
