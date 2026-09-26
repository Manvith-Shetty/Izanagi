//! On-chain bindings for Coinbase's deployed `x402BatchSettlement` escrow, the reads both the
//! agent and the seller need, and the EIP-1271 gate the escrow consults at claim time.

use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::sol;
use alloy::sol_types::SolValue;

sol! {
    /// Calldata form of the escrow's structs (the digest form lives in `crate::Voucher`).
    #[sol(rpc)]
    interface IX402BatchSettlement {
        struct ChannelConfig {
            address payer;
            address payerAuthorizer;
            address receiver;
            address receiverAuthorizer;
            address token;
            uint40  withdrawDelay;
            bytes32 salt;
        }
        struct Voucher { ChannelConfig channel; uint128 maxClaimableAmount; }
        struct VoucherClaim { Voucher voucher; bytes signature; uint128 totalClaimed; }

        /// Anyone may call this; the collector decides whose funds move. A Countersign
        /// wallet's collector pulls only into channels the wallet itself gates, and a wallet
        /// approves it only for the deposit it is making (`openTab`).
        function deposit(ChannelConfig calldata config, uint128 amount, address collector, bytes calldata collectorData) external;
        function claim(VoucherClaim[] calldata voucherClaims) external;
        function settle(address receiver, address token) external;
        function channels(bytes32) external view returns (uint128 balance, uint128 totalClaimed);
        /// Despite the name here, `finalizeAfter` holds when the withdrawal was STARTED:
        /// finishing reverts `WithdrawDelayNotElapsed` until the channel's `withdrawDelay` has
        /// passed since then (verified on Base mainnet).
        function pendingWithdrawals(bytes32) external view returns (uint128 amount, uint40 finalizeAfter);
    }

    #[sol(rpc)]
    interface IERC1271 {
        function isValidSignature(bytes32 hash, bytes signature) external view returns (bytes4);
    }

    /// A Countersign wallet. Counterparties read it to learn why they were refused; its
    /// guardians (the owner, and the risk oracle) flip the settlement-time kill switch.
    #[sol(rpc)]
    interface ICountersign {
        function revocations(address seller) external view returns (bool revoked, uint32 reason, uint64 revokedAt);
        function paused() external view returns (bool);
        function owner() external view returns (address);
        function agent() external view returns (address);
        function riskOracle() external view returns (address);
        function revoke(address seller, uint32 reason) external;
        function restore(address seller) external;
        function setPaused(bool p) external;

        // opening tabs (the agent), and what only the owner may do
        function collector() external view returns (address);
        function openTab(IX402BatchSettlement.ChannelConfig cfg, uint128 amount) external;
        function setCollector(address c) external;
        function transferOwnership(address newOwner) external;
        function initiateWithdraw(IX402BatchSettlement.ChannelConfig cfg, uint128 amount) external;
        function finalizeWithdraw(IX402BatchSettlement.ChannelConfig cfg) external;
        function sweep(address token, address to, uint256 amount) external;
    }

    /// The token calls a wallet's dashboard needs.
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address owner) external view returns (uint256);
        function allowance(address owner, address spender) external view returns (uint256);
    }
}

/// `isValidSignature` magic value.
pub const EIP1271_MAGIC: FixedBytes<4> = FixedBytes([0x16, 0x26, 0xba, 0x7e]);

impl From<&crate::ChannelConfig> for IX402BatchSettlement::ChannelConfig {
    fn from(c: &crate::ChannelConfig) -> Self {
        Self {
            payer: c.payer,
            payerAuthorizer: c.payerAuthorizer,
            receiver: c.receiver,
            receiverAuthorizer: c.receiverAuthorizer,
            token: c.token,
            withdrawDelay: c.withdrawDelay,
            salt: c.salt,
        }
    }
}

/// Intercepta trait names, by the compact code a Countersign wallet stores when it revokes.
/// 0 is "no trait", 99 is "a trait not listed here".
const REASONS: &[(u32, &str)] = &[
    (1, "sanction_address"),
    (2, "known_scammer"),
    (3, "rug_pull"),
    (4, "wallet_drainer"),
    (5, "phishing"),
    (6, "mixer"),
    (7, "honeypot"),
];

/// Intercepta trait name -> on-chain reason code.
pub fn reason_code(trait_name: &str) -> u32 {
    if trait_name.is_empty() {
        return 0;
    }
    REASONS.iter().find(|(_, n)| *n == trait_name).map(|(c, _)| *c).unwrap_or(99)
}

/// On-chain reason code -> Intercepta trait name.
pub fn reason_name(code: u32) -> &'static str {
    match code {
        0 => "unspecified",
        _ => REASONS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n).unwrap_or("other"),
    }
}

/// One claim row: cash `channel` in up to `total_claimed`, backed by a voucher for `max_claimable`.
pub fn claim_row(
    cfg: &crate::ChannelConfig,
    max_claimable: u128,
    signature: Bytes,
    total_claimed: u128,
) -> IX402BatchSettlement::VoucherClaim {
    IX402BatchSettlement::VoucherClaim {
        voucher: IX402BatchSettlement::Voucher { channel: cfg.into(), maxClaimableAmount: max_claimable },
        signature,
        totalClaimed: total_claimed,
    }
}

/// The parts of a Countersign signature blob a counterparty may want to read.
#[derive(Debug, Clone, PartialEq)]
pub struct CountersignBlob {
    pub agent_sig: Bytes,
    pub oracle_sig: Bytes,
    pub seller: Address,
    /// After this unix time the attestation lapses and the voucher stops being claimable.
    pub expiry: u64,
}

/// Decode `abi.encode(bytes agentSig, bytes oracleSig, address seller, uint64 expiry)`.
///
/// Returns `None` for any other signature format, e.g. a plain 65-byte ECDSA voucher.
pub fn decode_countersign_blob(sig: &[u8]) -> Option<CountersignBlob> {
    if sig.len() == 65 {
        return None;
    }
    let (agent_sig, oracle_sig, seller, expiry) =
        <(Bytes, Bytes, Address, U256)>::abi_decode_params(sig).ok()?;
    Some(CountersignBlob { agent_sig, oracle_sig, seller, expiry: expiry.try_into().ok()? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn countersign_blobs_round_trip() {
        let seller = address!("2222222222222222222222222222222222222222");
        let blob = crate::encode_signature_blob(&[1u8; 65], &[2u8; 65], seller, 1_800_000_000);
        let d = decode_countersign_blob(&blob).expect("a countersign blob");
        assert_eq!(d.seller, seller);
        assert_eq!(d.expiry, 1_800_000_000);
        assert_eq!(d.agent_sig.len(), 65);
    }

    #[test]
    fn reason_codes_round_trip() {
        for name in ["sanction_address", "wallet_drainer", "honeypot"] {
            assert_eq!(reason_name(reason_code(name)), name);
        }
        assert_eq!(reason_code(""), 0);
        assert_eq!(reason_code("something_new"), 99);
        assert_eq!(reason_name(99), "other");
    }

    #[test]
    fn plain_ecdsa_signatures_are_not_mistaken_for_blobs() {
        assert!(decode_countersign_blob(&[7u8; 65]).is_none());
        assert!(decode_countersign_blob(b"garbage").is_none());
    }
}
