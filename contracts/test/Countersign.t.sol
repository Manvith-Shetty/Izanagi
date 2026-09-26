// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test, console} from "forge-std/Test.sol";
import {Countersign, CountersignCollector} from "../src/Countersign.sol";
import {IX402BatchSettlement, IERC20, ChannelConfig, Voucher, VoucherClaim} from "../src/IX402.sol";

contract CountersignForkTest is Test {
    // REAL deployed Coinbase x402 batch-settlement escrow on Base mainnet
    address constant ESCROW = 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003;
    address constant USDC   = 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913;

    IX402BatchSettlement esc = IX402BatchSettlement(ESCROW);

    Countersign cs;
    CountersignCollector collector;

    uint256 agentPk  = 0xA11CE;
    uint256 oraclePk = 0x0DACE;
    uint256 rAuthPk  = 0xBEEF;
    uint256 evilPk   = 0xBAD;

    address agent;  address oracle;  address rAuth;
    address owner  = address(0x0117E5);
    address seller = address(0x5E11E5);

    ChannelConfig cfg;
    bytes32 channelId;
    uint128 constant DEPOSIT = 20_000_000; // 20 USDC

    function setUp() public {
        vm.createSelectFork(vm.envString("BASE_RPC"), 51777632);
        agent = vm.addr(agentPk); oracle = vm.addr(oraclePk); rAuth = vm.addr(rAuthPk);

        // these derived addresses can collide with real contracts on Base; force plain EOAs
        vm.etch(agent, ""); vm.etch(oracle, ""); vm.etch(rAuth, "");
        vm.etch(vm.addr(evilPk), ""); vm.etch(seller, "");

        collector = new CountersignCollector(ESCROW);
        cs = new Countersign(owner, ESCROW, agent, oracle);

        deal(USDC, address(cs), 100_000_000); // 100 USDC of REAL Base USDC

        cfg = ChannelConfig({
            payer: address(cs),
            payerAuthorizer: address(0),      // <-- routes voucher validation to EIP-1271 on payer
            receiver: seller,
            receiverAuthorizer: rAuth,
            token: USDC,
            withdrawDelay: 15 minutes,
            salt: bytes32(uint256(1))
        });

        vm.startPrank(owner);
        cs.approveToken(USDC, address(collector), type(uint256).max);
        cs.openChannel(cfg, DEPOSIT, address(collector));
        vm.stopPrank();

        channelId = esc.getChannelId(cfg);
    }

    // ---------- helpers ----------
    function _sig(uint256 pk, bytes32 h) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, h);
        return abi.encodePacked(r, s, v);
    }
    function _blob(uint256 aPk, uint256 oPk, bytes32 digest, address sellerAddr, uint64 expiry)
        internal view returns (bytes memory)
    {
        bytes32 att = keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(), digest, sellerAddr, expiry));
        return abi.encode(_sig(aPk, digest), _sig(oPk, att), sellerAddr, expiry);
    }
    function _claimRow(uint128 ceiling, uint128 total, bytes memory blob)
        internal view returns (VoucherClaim[] memory rows)
    {
        rows = new VoucherClaim[](1);
        rows[0] = VoucherClaim({
            voucher: Voucher({channel: cfg, maxClaimableAmount: ceiling}),
            signature: blob,
            totalClaimed: total
        });
    }
    function _submit(VoucherClaim[] memory rows) internal {
        bytes memory a = _authSig(rows);
        esc.claimWithSignature(rows, a);
    }
    function _authSig(VoucherClaim[] memory rows) internal view returns (bytes memory) {
        return _sig(rAuthPk, esc.getClaimBatchDigest(rows));
    }
    /// compute everything first, THEN expectRevert, THEN the real call
    function _expectRevertSubmit(VoucherClaim[] memory rows) internal {
        bytes memory a = _authSig(rows);
        vm.expectRevert();
        esc.claimWithSignature(rows, a);
    }
    function _std(uint128 ceiling, uint128 total) internal view returns (VoucherClaim[] memory) {
        bytes32 d = esc.getVoucherDigest(channelId, ceiling);
        return _claimRow(ceiling, total, _blob(agentPk, oraclePk, d, seller, uint64(block.timestamp + 1 hours)));
    }

    // ===================== 1. INFRASTRUCTURE =====================
    function test_01_RealEscrowIsDeployed() public view {
        assertGt(ESCROW.code.length, 10000, "escrow bytecode");
        (uint128 bal,) = esc.channels(channelId);
        assertEq(bal, DEPOSIT, "real escrow holds our deposit");
        console.log("escrow code bytes:", ESCROW.code.length);
        console.log("channel balance (USDC 6dp):", uint256(bal));
    }

    // ===================== 2. HAPPY PATH =====================
    function test_02_HappyPath_2of2_ClaimsOnRealEscrow() public {
        uint256 g0 = gasleft();
        _submit(_std(5_000_000, 5_000_000));
        uint256 used = g0 - gasleft();
        (, uint128 claimed) = esc.channels(channelId);
        assertEq(claimed, 5_000_000, "5 USDC claimed");
        esc.settle(seller, USDC);
        assertEq(IERC20(USDC).balanceOf(seller), 5_000_000, "seller paid real USDC");
        console.log("gas for claimWithSignature via EIP-1271:", used);
    }

    // ===================== 3. ATTACK / FAILURE PATHS =====================
    function test_03_Revert_AgentSignatureAlone() public {
        bytes32 d = esc.getVoucherDigest(channelId, 5_000_000);
        // oracle slot filled with the AGENT's signature => oracle recover fails
        bytes memory blob = abi.encode(_sig(agentPk, d), _sig(agentPk, d), seller, uint64(block.timestamp + 1 hours));
        _expectRevertSubmit(_claimRow(5_000_000, 5_000_000, blob));
    }
    function test_04_Revert_WrongOracleKey() public {
        bytes32 d = esc.getVoucherDigest(channelId, 5_000_000);
        _expectRevertSubmit(_claimRow(5_000_000, 5_000_000, _blob(agentPk, evilPk, d, seller, uint64(block.timestamp + 1 hours))));
    }
    function test_05_Revert_CompromisedAgentKey() public {
        bytes32 d = esc.getVoucherDigest(channelId, 5_000_000);
        _expectRevertSubmit(_claimRow(5_000_000, 5_000_000, _blob(evilPk, oraclePk, d, seller, uint64(block.timestamp + 1 hours))));
    }
    function test_06_Revert_ExpiredAttestation() public {
        bytes32 d = esc.getVoucherDigest(channelId, 5_000_000);
        bytes memory blob = _blob(agentPk, oraclePk, d, seller, uint64(block.timestamp + 60));
        vm.warp(block.timestamp + 120);
        _expectRevertSubmit(_claimRow(5_000_000, 5_000_000, blob));
    }
    function test_07_Revert_AttestationBoundToDifferentSeller() public {
        bytes32 d = esc.getVoucherDigest(channelId, 5_000_000);
        // oracle attested for a DIFFERENT seller; claim routes to our real seller
        bytes32 att = keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(), d, address(0xDEAD), uint64(block.timestamp + 1 hours)));
        bytes memory blob = abi.encode(_sig(agentPk, d), _sig(oraclePk, att), seller, uint64(block.timestamp + 1 hours));
        _expectRevertSubmit(_claimRow(5_000_000, 5_000_000, blob));
    }
    function test_08_Revert_OverCeiling() public {
        bytes32 d = esc.getVoucherDigest(channelId, 5_000_000);
        bytes memory blob = _blob(agentPk, oraclePk, d, seller, uint64(block.timestamp + 1 hours));
        _expectRevertSubmit(_claimRow(5_000_000, 6_000_000, blob)); // totalClaimed > maxClaimableAmount
    }
    function test_09_Revert_OverEscrowBalance() public {
        _expectRevertSubmit(_std(50_000_000, 50_000_000)); // ceiling above deposited balance
    }
    function test_10_Replay_IsNoOp_NotDoubleSpend() public {
        _submit(_std(5_000_000, 5_000_000));
        (, uint128 c1) = esc.channels(channelId);
        _submit(_std(5_000_000, 5_000_000)); // replay identical voucher
        (, uint128 c2) = esc.channels(channelId);
        assertEq(c1, c2, "replay did not increase totalClaimed");
        esc.settle(seller, USDC);
        assertEq(IERC20(USDC).balanceOf(seller), 5_000_000, "seller paid once");
    }

    // ===================== 4. THE DIFFERENTIATOR =====================
    function test_11_Revoked_SellerCannotClaim() public {
        vm.prank(oracle);
        cs.revoke(seller, 1);
        _expectRevertSubmit(_std(5_000_000, 5_000_000));
    }

    /// The money shot: agent already paid 20 times off-chain. Seller flagged AFTER signing,
    /// BEFORE settlement. Every signed voucher becomes worthless.
    function test_12_RevocationAfterSigning_KillsAlreadySignedVouchers() public {
        // 20 off-chain vouchers, cumulative ceiling grows to 10 USDC
        bytes[] memory blobs = new bytes[](20);
        for (uint128 i = 1; i <= 20; i++) {
            uint128 ceiling = i * 500_000; // 0.5 USDC per call
            bytes32 d = esc.getVoucherDigest(channelId, ceiling);
            blobs[i-1] = _blob(agentPk, oraclePk, d, seller, uint64(block.timestamp + 1 hours));
        }
        // seller could settle the highest voucher right now:
        uint256 snap = vm.snapshotState();
        _submit(_claimRow(10_000_000, 10_000_000, blobs[19]));
        (, uint128 wouldHave) = esc.channels(channelId);
        assertEq(wouldHave, 10_000_000, "without revocation the seller gets 10 USDC");
        vm.revertToState(snap);

        // ...but the score moves and the oracle revokes mid-session
        vm.prank(oracle);
        cs.revoke(seller, 1);

        _expectRevertSubmit(_claimRow(10_000_000, 10_000_000, blobs[19]));

        (, uint128 claimed) = esc.channels(channelId);
        assertEq(claimed, 0, "20 already-signed vouchers are now worthless");
        console.log("vouchers signed before revocation:", uint256(20));
        console.log("USDC the seller would have taken:", uint256(10_000_000));
        console.log("USDC actually claimable after revocation:", uint256(claimed));
    }

    function test_13_UnrevokeRestoresClaimability() public {
        vm.startPrank(oracle);
        cs.revoke(seller, 1);
        cs.restore(seller);
        vm.stopPrank();
        _submit(_std(5_000_000, 5_000_000));
        (, uint128 c) = esc.channels(channelId);
        assertEq(c, 5_000_000);
    }
    function test_14_Revert_AgentCannotRevoke() public {
        vm.prank(agent);
        vm.expectRevert();
        cs.revoke(seller, 1);
    }

    // ===================== 5. ESCAPE HATCH =====================
    function test_15_EscapeHatch_OwnerRecoversWithoutOracle() public {
        uint256 before = IERC20(USDC).balanceOf(address(cs));
        vm.startPrank(owner);
        cs.initiateWithdraw(cfg, DEPOSIT);
        vm.warp(block.timestamp + 15 minutes + 1);
        cs.finalizeWithdraw(cfg);
        vm.stopPrank();
        assertEq(IERC20(USDC).balanceOf(address(cs)), before + DEPOSIT, "funds returned with no oracle");
    }
    function test_16_Revert_AgentCannotWithdraw() public {
        vm.prank(agent);
        vm.expectRevert();
        cs.initiateWithdraw(cfg, DEPOSIT);
    }
    function test_17_Revert_OracleCannotWithdraw() public {
        vm.prank(oracle);
        vm.expectRevert();
        cs.initiateWithdraw(cfg, DEPOSIT);
    }
}
