// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IX402BatchSettlement, IDepositCollector, IERC20, ChannelConfig} from "./IX402.sol";

/// @title CountersignCollector
/// @notice Pluggable deposit collector: pulls funds from the Countersign wallet into the escrow.
/// @dev The escrow calls this, so `msg.sender` is the escrow itself.
contract CountersignCollector is IDepositCollector {
    function collect(address payer, address token, uint256 amount, bytes32, bytes calldata) external {
        require(IERC20(token).transferFrom(payer, msg.sender, amount), "pull failed");
    }
}

/// @title Countersign
/// @notice A policy-gated EIP-1271 payer for x402 `batch-settlement` payment channels.
///
/// @dev Countersign is the `payer` of an x402 channel whose `payerAuthorizer` is `address(0)`.
///      That configuration makes Coinbase's deployed `x402BatchSettlement` escrow validate every
///      voucher through `SignatureChecker.isValidSignatureNow(payer, ...)` -- i.e. through this
///      contract -- at CLAIM time rather than at signing time.
///
///      A voucher is only claimable if ALL of the following hold *when the seller cashes it in*:
///        1. the agent signed the voucher digest,
///        2. the risk oracle countersigned (digest, seller, expiry),
///        3. that attestation has not expired,
///        4. the seller is not revoked,
///        5. the wallet is not paused.
///
///      Conditions 4 and 5 are read from storage during validation. Because they are evaluated at
///      claim time, a seller that turns malicious AFTER the agent has already paid -- but before
///      settlement -- can still be stopped. Every other agent-payment system validates at signing
///      time, where the signature is already final.
///
///      Trust model: the oracle holds a veto, never the funds. It cannot move money (the agent must
///      also sign), cannot redirect it (the digest binds the channel, which binds the receiver), and
///      cannot trap it (`initiateWithdraw`/`finalizeWithdraw` are owner-only and need no oracle).
contract Countersign {
    /*//////////////////////////////////////////////////////////////
                                CONSTANTS
    //////////////////////////////////////////////////////////////*/

    bytes4 private constant MAGIC = 0x1626ba7e;
    bytes4 private constant INVALID = 0xffffffff;

    /// @dev secp256k1 curve order / 2; signatures with higher `s` are malleable duplicates.
    uint256 private constant HALF_N = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    bytes32 public constant ATTESTATION_TYPEHASH =
        keccak256("Attestation(bytes32 digest,address seller,uint64 expiry)");

    /*//////////////////////////////////////////////////////////////
                                 STORAGE
    //////////////////////////////////////////////////////////////*/

    address public immutable owner; // the human
    address public immutable escrow; // Coinbase x402BatchSettlement

    address public agent; // agent hot key (may be compromised; alone it can do nothing)
    address public riskOracle; // backend countersigner (holds a veto, never the funds)

    bool public paused;

    struct Revocation {
        bool revoked;
        uint32 reason; // Intercepta trait code, for demo/audit visibility
        uint64 revokedAt; // when the kill switch was flipped
    }

    /// @notice Settlement-time kill switch, keyed by seller.
    mapping(address => Revocation) public revocations;

    /*//////////////////////////////////////////////////////////////
                             ERRORS & EVENTS
    //////////////////////////////////////////////////////////////*/

    error NotOwner();
    error NotGuardian();

    event AgentChanged(address indexed agent);
    event OracleChanged(address indexed oracle);
    event PausedSet(bool paused);
    event SellerRevoked(address indexed seller, uint32 reason, uint64 revokedAt);
    event SellerRestored(address indexed seller);
    event ChannelOpened(bytes32 indexed channelId, address indexed seller, uint128 amount);

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    constructor(address _owner, address _escrow, address _agent, address _oracle) {
        owner = _owner;
        escrow = _escrow;
        agent = _agent;
        riskOracle = _oracle;
    }

    /*//////////////////////////////////////////////////////////////
                                 ADMIN
    //////////////////////////////////////////////////////////////*/

    function setAgent(address a) external onlyOwner {
        agent = a;
        emit AgentChanged(a);
    }

    function setOracle(address o) external onlyOwner {
        riskOracle = o;
        emit OracleChanged(o);
    }

    function setPaused(bool p) external {
        if (msg.sender != owner && msg.sender != riskOracle) revert NotGuardian();
        paused = p;
        emit PausedSet(p);
    }

    function approveToken(address token, address spender, uint256 amount) external onlyOwner {
        IERC20(token).approve(spender, amount);
    }

    /*//////////////////////////////////////////////////////////////
                      THE SETTLEMENT-TIME KILL SWITCH
    //////////////////////////////////////////////////////////////*/

    /// @notice Revoke a seller. Every voucher already signed for them becomes unclaimable.
    /// @dev Callable by the risk oracle (automated, when a score moves) or the owner (manual).
    ///      This is the one operation no pre-signature guard can offer.
    function revoke(address seller, uint32 reason) external {
        if (msg.sender != owner && msg.sender != riskOracle) revert NotGuardian();
        revocations[seller] = Revocation({revoked: true, reason: reason, revokedAt: uint64(block.timestamp)});
        emit SellerRevoked(seller, reason, uint64(block.timestamp));
    }

    /// @notice Clear a revocation (score recovered, or a false positive).
    function restore(address seller) external {
        if (msg.sender != owner && msg.sender != riskOracle) revert NotGuardian();
        delete revocations[seller];
        emit SellerRestored(seller);
    }

    function isRevoked(address seller) external view returns (bool) {
        return revocations[seller].revoked;
    }

    /*//////////////////////////////////////////////////////////////
                            CHANNEL LIFECYCLE
    //////////////////////////////////////////////////////////////*/

    function openChannel(ChannelConfig calldata cfg, uint128 amount, address collector) external onlyOwner {
        IX402BatchSettlement(escrow).deposit(cfg, amount, collector, "");
        emit ChannelOpened(IX402BatchSettlement(escrow).getChannelId(cfg), cfg.receiver, amount);
    }

    /// @notice Escape hatch. Owner only -- never the agent, never the oracle.
    /// @dev If the oracle disappears forever, the owner still recovers the escrow after
    ///      `cfg.withdrawDelay` (15 minutes minimum) with no oracle involvement at all.
    function initiateWithdraw(ChannelConfig calldata cfg, uint128 amount) external onlyOwner {
        IX402BatchSettlement(escrow).initiateWithdraw(cfg, amount);
    }

    function finalizeWithdraw(ChannelConfig calldata cfg) external onlyOwner {
        IX402BatchSettlement(escrow).finalizeWithdraw(cfg);
    }

    function sweep(address token, address to, uint256 amount) external onlyOwner {
        IERC20(token).transfer(to, amount);
    }

    /*//////////////////////////////////////////////////////////////
                         EIP-1271: THE POLICY GATE
    //////////////////////////////////////////////////////////////*/

    /// @notice Called by the x402 escrow for every voucher, at claim time.
    /// @param digest    EIP-712 `Voucher(bytes32 channelId,uint128 maxClaimableAmount)` digest.
    /// @param signature abi.encode(bytes agentSig, bytes oracleSig, address seller, uint64 expiry)
    /// @return MAGIC (0x1626ba7e) if the voucher may be claimed right now, else INVALID.
    function isValidSignature(bytes32 digest, bytes calldata signature) external view returns (bytes4) {
        (bytes memory agentSig, bytes memory oracleSig, address seller, uint64 expiry) =
            abi.decode(signature, (bytes, bytes, address, uint64));

        // 1. the agent must have authorised this exact voucher
        if (_recover(digest, agentSig) != agent) return INVALID;

        // 2. the risk oracle must have countersigned it for this specific seller
        bytes32 attestation = keccak256(abi.encode(ATTESTATION_TYPEHASH, digest, seller, expiry));
        if (_recover(attestation, oracleSig) != riskOracle) return INVALID;

        // 3. the verdict must still be fresh
        if (block.timestamp > expiry) return INVALID;

        // --- everything above could be checked at signing time. Everything below could not. ---

        // 4. the seller must not have been revoked since the agent paid
        if (revocations[seller].revoked) return INVALID;

        // 5. global stop
        if (paused) return INVALID;

        return MAGIC;
    }

    /// @dev Malleability-safe ECDSA recovery (rejects high-`s` and bad `v`).
    function _recover(bytes32 hash, bytes memory sig) internal pure returns (address) {
        if (sig.length != 65) return address(0);
        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(sig, 0x20))
            s := mload(add(sig, 0x40))
            v := byte(0, mload(add(sig, 0x60)))
        }
        if (uint256(s) > HALF_N) return address(0);
        if (v < 27) v += 27;
        if (v != 27 && v != 28) return address(0);
        return ecrecover(hash, v, r, s);
    }
}
