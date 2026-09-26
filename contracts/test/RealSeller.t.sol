// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test, console} from "forge-std/Test.sol";
import {Countersign, CountersignCollector} from "../src/Countersign.sol";
import {IX402BatchSettlement, IERC20, ChannelConfig, Voucher, VoucherClaim} from "../src/IX402.sol";

/// @title RealSellerForkTest
/// @notice A Countersign payer against the channel parameters of two REAL, unaffiliated
///         x402 `batch-settlement` sellers found on Coinbase's live Bazaar:
///
///           hyperextend  api.hyperextend.xyz   payTo 0x548f...abc4  rAuth 0x3721...c0Fb
///           onesource    api.onesource.io      payTo 0x52E2...f3ea  rAuth 0x11dF...f81b
///
///         Every address, price and withdrawDelay below was read from each seller's live
///         HTTP 402 response on 2026-09-26 — nothing is invented. The only simulated part
///         is `vm.prank(payTo)`: on a fork we cannot hold the seller's key, so we submit
///         the claim *as* them, exercising exactly the code path their claimer runs.
contract RealSellerForkTest is Test {
    address constant ESCROW = 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003;
    address constant USDC   = 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913;

    // hyperextend — live 402 on https://api.hyperextend.xyz/v1/candles/BTC/1m/latest
    address constant HX_PAYTO = 0x548fC289526ab2F0391D562a723cdA64Bbf1abc4;
    address constant HX_RAUTH = 0x3721824a31197dcDD2984cF43b92B6cc8A87c0Fb;
    uint128 constant HX_PRICE = 2000; // 0.002 USDC per call, their real price

    // onesource — live 402 on https://api.onesource.io/api/chain/block-number
    address constant OS_PAYTO = 0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea;
    address constant OS_RAUTH = 0x11dF9F6280632aB8F12926b3f569E493EaEcf81b;
    uint128 constant OS_PRICE = 1000; // 0.001 USDC per call

    uint40 constant REAL_DELAY = 86400; // both sellers advertise withdrawDelay = 1 day

    IX402BatchSettlement esc = IX402BatchSettlement(ESCROW);

    Countersign cs;
    CountersignCollector collector;

    uint256 agentPk  = 0xA11CE;
    uint256 oraclePk = 0x0DACE;
    address agent;
    address oracle;
    address owner = address(0x0117E5);

    function setUp() public {
        vm.createSelectFork(vm.envString("BASE_RPC"));
        agent = vm.addr(agentPk);
        oracle = vm.addr(oraclePk);
        vm.etch(agent, "");
        vm.etch(oracle, "");

        collector = new CountersignCollector();
        cs = new Countersign(owner, ESCROW, agent, oracle);
        deal(USDC, address(cs), 20_000_000); // 20 real Base USDC

        vm.prank(owner);
        cs.approveToken(USDC, address(collector), type(uint256).max);
    }

    // ---------- helpers ----------

    function _cfg(address payTo, address rAuth) internal view returns (ChannelConfig memory) {
        return ChannelConfig({
            payer: address(cs),
            payerAuthorizer: address(0), // routes voucher validation to Countersign.isValidSignature
            receiver: payTo,
            receiverAuthorizer: rAuth,
            token: USDC,
            withdrawDelay: REAL_DELAY,
            salt: bytes32(uint256(0x7ab))
        });
    }

    function _sig(uint256 pk, bytes32 h) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, h);
        return abi.encodePacked(r, s, v);
    }

    function _rows(ChannelConfig memory cfg, uint128 ceiling, uint128 total)
        internal view returns (VoucherClaim[] memory rows)
    {
        bytes32 digest = esc.getVoucherDigest(esc.getChannelId(cfg), ceiling);
        uint64 expiry = uint64(block.timestamp + 1 hours);
        bytes32 att = keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(), digest, cfg.receiver, expiry));
        bytes memory blob = abi.encode(_sig(agentPk, digest), _sig(oraclePk, att), cfg.receiver, expiry);
        rows = new VoucherClaim[](1);
        rows[0] = VoucherClaim({
            voucher: Voucher({channel: cfg, maxClaimableAmount: ceiling}),
            signature: blob,
            totalClaimed: total
        });
    }

    /// One seller's whole product journey: pay → they claim → they turn bad → they get nothing.
    function _journey(string memory name, address payTo, address rAuth, uint128 price) internal {
        ChannelConfig memory cfg = _cfg(payTo, rAuth);

        // 1. open a channel to the REAL seller and escrow real USDC for it
        vm.prank(owner);
        cs.openChannel(cfg, 10_000_000, address(collector));

        // 2. ten paid calls happen off-chain; the running voucher reaches 10x their price
        uint128 ceiling = price * 10;

        // 3. the seller cashes in, exactly as their claimer would (direct `claim` needs
        //    msg.sender on the receiver side — that is the seller's own code path)
        uint256 before = IERC20(USDC).balanceOf(payTo);
        VoucherClaim[] memory rows1 = _rows(cfg, ceiling, ceiling); // build BEFORE prank
        vm.prank(payTo);
        esc.claim(rows1);
        esc.settle(payTo, USDC); // anyone may push settlement of already-claimed funds
        // >= not ==: settle() pays the receiver across ALL their channels, and a live seller
        // (onesource) has real pending claims from real payers that sweep along with ours.
        assertGe(IERC20(USDC).balanceOf(payTo) - before, ceiling, "seller was not paid");
        console.log("%s claimed and settled (real USDC):", name, ceiling);

        // 4. ten MORE calls are signed... and then the seller turns malicious
        uint128 ceiling2 = ceiling + price * 10;
        VoucherClaim[] memory rows2 = _rows(cfg, ceiling2, ceiling2);
        vm.prank(oracle);
        cs.revoke(payTo, 2); // known_scammer

        // 5. the already-signed voucher is now worthless — the escrow itself refuses it
        vm.prank(payTo);
        vm.expectRevert();
        esc.claim(rows2);
        console.log("%s revoked after signing: claim reverted", name);

        // 6. false positive? restore, and the same voucher works again
        vm.prank(oracle);
        cs.restore(payTo);
        vm.prank(payTo);
        esc.claim(rows2);
        esc.settle(payTo, USDC);
        assertGe(IERC20(USDC).balanceOf(payTo) - before, ceiling2, "post-restore claim failed");
        console.log("%s restored: same voucher claimable again", name);
    }

    function test_RealSeller_Hyperextend() public {
        _journey("hyperextend", HX_PAYTO, HX_RAUTH, HX_PRICE);
    }

    function test_RealSeller_OneSource() public {
        _journey("onesource", OS_PAYTO, OS_RAUTH, OS_PRICE);
    }

    /// The sellers' real receiverAuthorizer keys can also claim via claimWithSignature —
    /// we cannot produce their signature, and nobody else can either: a forged authorizer
    /// signature must revert. (This is the escrow protecting THEM, shown for completeness.)
    function test_RealSeller_ForgedAuthorizerSignatureReverts() public {
        ChannelConfig memory cfg = _cfg(HX_PAYTO, HX_RAUTH);
        vm.prank(owner);
        cs.openChannel(cfg, 1_000_000, address(collector));
        VoucherClaim[] memory rows = _rows(cfg, HX_PRICE, HX_PRICE);
        bytes memory forged = _sig(uint256(0xF0553), esc.getClaimBatchDigest(rows));
        vm.expectRevert();
        esc.claimWithSignature(rows, forged);
    }
}
