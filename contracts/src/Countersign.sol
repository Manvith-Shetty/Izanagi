// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IX402BatchSettlement, IDepositCollector, IERC20, ChannelConfig} from "./IX402.sol";

/// @title CountersignCollector
/// @notice Pluggable deposit collector: pulls funds from a Countersign wallet into the escrow.
/// @dev Wallets approve this contract, so whoever can make it call `transferFrom` can spend
///      their allowance. Only the escrow may, and the funds only ever go to the escrow -- into a
///      channel whose vouchers the wallet itself validates, withdrawable only by its owner.
///
///      That last part is checked here, not assumed: the escrow's `deposit` is open to anyone,
///      and a channel names its own `payerAuthorizer`. Without the check a stranger could open a
///      channel funded by the wallet, name themselves as its authorizer, sign their own vouchers
///      and claim the lot. So the depositor must pass the channel config as `collectorData`,
///      and it must be this wallet's, gated by the wallet (`payerAuthorizer == 0`).
contract CountersignCollector is IDepositCollector {
    address public immutable escrow;

    error NotEscrow();
    error WrongChannel();
    error NotGatedByPayer();

    constructor(address _escrow) {
        escrow = _escrow;
    }

    function collect(address payer, address token, uint256 amount, bytes32 channelId, bytes calldata data) external {
        if (msg.sender != escrow) revert NotEscrow();
        ChannelConfig memory cfg = abi.decode(data, (ChannelConfig));
        if (IX402BatchSettlement(escrow).getChannelId(cfg) != channelId) revert WrongChannel();
        if (cfg.payer != payer || cfg.token != token || cfg.payerAuthorizer != address(0)) revert NotGatedByPayer();
        require(IERC20(token).transferFrom(payer, escrow, amount), "pull failed");
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
///
///      Ownership can be handed over once per owner, and only by the current owner: a free trial
///      starts owned by the operator, and passes to the person when they add their own money.
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

    /// @notice Most time the agent may lock funds in a tab for. Real sellers ask for a day.
    uint40 public constant MAX_TAB_DELAY = 7 days;

    address public owner; // the human (the operator during a free trial, until they take over)
    address public immutable escrow; // Coinbase x402BatchSettlement
    address public collector; // the deposit collector the agent opens tabs through

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
    error NotAgent();
    error ZeroOwner();
    error NoCollector();
    error BadChannel();

    event OwnerChanged(address indexed previousOwner, address indexed newOwner);
    event CollectorChanged(address indexed collector);
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

    /// @notice Hand the wallet to a new owner. Every owner power moves with it: withdrawals,
    ///         keys, the collector. The previous owner keeps none of them.
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroOwner();
        emit OwnerChanged(owner, newOwner);
        owner = newOwner;
    }

    function setCollector(address c) external onlyOwner {
        collector = c;
        emit CollectorChanged(c);
    }

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

    function openChannel(ChannelConfig calldata cfg, uint128 amount, address _collector) external onlyOwner {
        IX402BatchSettlement(escrow).deposit(cfg, amount, _collector, abi.encode(cfg));
        emit ChannelOpened(IX402BatchSettlement(escrow).getChannelId(cfg), cfg.receiver, amount);
    }

    /// @notice Open (or top up) a tab from this wallet's own funds. The agent may: it spends
    ///         nothing, since the money stays in a channel only this wallet's vouchers unlock.
    /// @dev The collector is approved for exactly `amount` and reset after, so no allowance is
    ///      left standing between tabs for anyone else to use.
    function openTab(ChannelConfig calldata cfg, uint128 amount) external {
        if (msg.sender != agent && msg.sender != owner) revert NotAgent();
        if (cfg.payer != address(this) || cfg.payerAuthorizer != address(0) || cfg.withdrawDelay > MAX_TAB_DELAY) {
            revert BadChannel();
        }
        address c = collector;
        if (c == address(0)) revert NoCollector();
        IERC20(cfg.token).approve(c, amount);
        IX402BatchSettlement(escrow).deposit(cfg, amount, c, abi.encode(cfg));
        IERC20(cfg.token).approve(c, 0);
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
