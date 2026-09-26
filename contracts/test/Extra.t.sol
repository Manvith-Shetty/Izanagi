// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test, console} from "forge-std/Test.sol";
import {Countersign, CountersignCollector} from "../src/Countersign.sol";
import {IX402BatchSettlement, IERC20, ChannelConfig, Voucher, VoucherClaim} from "../src/IX402.sol";

contract ExtraTest is Test {
    address constant ESCROW  = 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003;
    address constant USDC    = 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913;
    address constant CREATE2 = 0x4e59b44847b379578588920cA78FbF26c0B4956C; // canonical deterministic deployer
    // REAL x402 seller on Base mainnet (354 of the 379 live channels pay this address)
    address constant REAL_SELLER = 0xdF1b6e9Ae298A1C65193831A05C56cea4c3343A9;

    IX402BatchSettlement esc = IX402BatchSettlement(ESCROW);
    uint256 agentPk = 0xA11CE; uint256 oraclePk = 0x0DACE; uint256 rAuthPk = 0xBEEF;
    address agent; address oracle; address rAuth; address owner = address(0x0117E5);

    function setUp() public { vm.createSelectFork(vm.envString("BASE_RPC"), 51777632); }

    function _sig(uint256 pk, bytes32 h) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, h); return abi.encodePacked(r,s,v);
    }

    /// The REAL deployment path: canonical CREATE2 deployer, not `new`.
    function test_A_DeployViaRealCREATE2Deployer() public {
        assertGt(CREATE2.code.length, 0, "canonical CREATE2 deployer is live on Base");
        bytes32 salt = keccak256("countersign.v1");
        bytes memory initcode = abi.encodePacked(
            type(Countersign).creationCode,
            abi.encode(owner, ESCROW, vm.addr(agentPk), vm.addr(oraclePk))
        );
        address predicted = address(uint160(uint256(keccak256(
            abi.encodePacked(bytes1(0xff), CREATE2, salt, keccak256(initcode))))));
        (bool ok,) = CREATE2.call(abi.encodePacked(salt, initcode));
        assertTrue(ok, "CREATE2 deploy");
        assertGt(predicted.code.length, 0, "deployed at predicted address");
        assertEq(Countersign(predicted).escrow(), ESCROW);
        console.log("CREATE2 deployer code bytes:", CREATE2.code.length);
        console.log("Countersign runtime bytes:  ", predicted.code.length);
        console.log("deterministic address:      ", predicted);
    }

    /// Pay a REAL mainnet x402 seller (not one we invented).
    function test_B_PaysRealMainnetSeller() public {
        agent = vm.addr(agentPk); oracle = vm.addr(oraclePk); rAuth = vm.addr(rAuthPk);
        vm.etch(agent,""); vm.etch(oracle,""); vm.etch(rAuth,"");
        CountersignCollector col = new CountersignCollector(ESCROW);
        Countersign cs = new Countersign(owner, ESCROW, agent, oracle);
        deal(USDC, address(cs), 50_000_000);

        ChannelConfig memory cfg = ChannelConfig({
            payer: address(cs), payerAuthorizer: address(0),
            receiver: REAL_SELLER, receiverAuthorizer: rAuth,
            token: USDC, withdrawDelay: 15 minutes, salt: bytes32(uint256(7))
        });
        vm.startPrank(owner);
        cs.approveToken(USDC, address(col), type(uint256).max);
        cs.openChannel(cfg, 10_000_000, address(col));
        vm.stopPrank();

        bytes32 cid = esc.getChannelId(cfg);
        bytes32 d = esc.getVoucherDigest(cid, 3_000_000);
        uint64 exp = uint64(block.timestamp + 1 hours);
        bytes32 att = keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(), d, REAL_SELLER, exp));
        bytes memory blob = abi.encode(_sig(agentPk,d), _sig(oraclePk,att), REAL_SELLER, exp);

        VoucherClaim[] memory rows = new VoucherClaim[](1);
        rows[0] = VoucherClaim({voucher: Voucher({channel: cfg, maxClaimableAmount: 3_000_000}), signature: blob, totalClaimed: 3_000_000});

        uint256 balBefore = IERC20(USDC).balanceOf(REAL_SELLER);
        esc.claimWithSignature(rows, _sig(rAuthPk, esc.getClaimBatchDigest(rows)));
        esc.settle(REAL_SELLER, USDC);
        assertEq(IERC20(USDC).balanceOf(REAL_SELLER) - balBefore, 3_000_000, "real seller received real USDC");
        console.log("real seller USDC before:", balBefore);
        console.log("real seller USDC after :", IERC20(USDC).balanceOf(REAL_SELLER));
    }

    /// LIMITATION: the seller can claim directly with no receiverAuthorizer signature.
    /// Our EIP-1271 gate still applies, so revocation still binds.
    function test_C_DirectClaimBySeller_StillGated() public {
        agent = vm.addr(agentPk); oracle = vm.addr(oraclePk); rAuth = vm.addr(rAuthPk);
        vm.etch(agent,""); vm.etch(oracle,""); vm.etch(rAuth,"");
        address seller = address(0x5E11E5); vm.etch(seller,"");
        CountersignCollector col = new CountersignCollector(ESCROW);
        Countersign cs = new Countersign(owner, ESCROW, agent, oracle);
        deal(USDC, address(cs), 50_000_000);
        ChannelConfig memory cfg = ChannelConfig({
            payer: address(cs), payerAuthorizer: address(0), receiver: seller,
            receiverAuthorizer: rAuth, token: USDC, withdrawDelay: 15 minutes, salt: bytes32(uint256(9))});
        vm.startPrank(owner);
        cs.approveToken(USDC, address(col), type(uint256).max);
        cs.openChannel(cfg, 10_000_000, address(col));
        vm.stopPrank();

        bytes32 cid = esc.getChannelId(cfg);
        bytes32 d = esc.getVoucherDigest(cid, 2_000_000);
        uint64 exp = uint64(block.timestamp + 1 hours);
        bytes32 att = keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(), d, seller, exp));
        bytes memory blob = abi.encode(_sig(agentPk,d), _sig(oraclePk,att), seller, exp);
        VoucherClaim[] memory rows = new VoucherClaim[](1);
        rows[0] = VoucherClaim({voucher: Voucher({channel: cfg, maxClaimableAmount: 2_000_000}), signature: blob, totalClaimed: 2_000_000});

        // seller claims directly, no receiverAuthorizer signature needed
        vm.prank(seller);
        esc.claim(rows);
        (, uint128 c) = esc.channels(cid);
        assertEq(c, 2_000_000, "direct claim works for the receiver");

        // but revocation still gates a direct claim
        vm.prank(oracle); cs.revoke(seller, 1);
        bytes32 d2 = esc.getVoucherDigest(cid, 4_000_000);
        bytes32 att2 = keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(), d2, seller, exp));
        rows[0] = VoucherClaim({voucher: Voucher({channel: cfg, maxClaimableAmount: 4_000_000}),
            signature: abi.encode(_sig(agentPk,d2), _sig(oraclePk,att2), seller, exp), totalClaimed: 4_000_000});
        vm.prank(seller);
        vm.expectRevert();
        esc.claim(rows);
    }

    /// LIMITATION: revocation only binds BEFORE the claim lands. Once claimed, it is final.
    function test_D_Limitation_ClaimBeforeRevokeIsFinal() public {
        agent = vm.addr(agentPk); oracle = vm.addr(oraclePk); rAuth = vm.addr(rAuthPk);
        vm.etch(agent,""); vm.etch(oracle,""); vm.etch(rAuth,"");
        address seller = address(0x5E11E5); vm.etch(seller,"");
        CountersignCollector col = new CountersignCollector(ESCROW);
        Countersign cs = new Countersign(owner, ESCROW, agent, oracle);
        deal(USDC, address(cs), 50_000_000);
        ChannelConfig memory cfg = ChannelConfig({
            payer: address(cs), payerAuthorizer: address(0), receiver: seller,
            receiverAuthorizer: rAuth, token: USDC, withdrawDelay: 15 minutes, salt: bytes32(uint256(11))});
        vm.startPrank(owner);
        cs.approveToken(USDC, address(col), type(uint256).max);
        cs.openChannel(cfg, 10_000_000, address(col));
        vm.stopPrank();
        bytes32 cid = esc.getChannelId(cfg);
        bytes32 d = esc.getVoucherDigest(cid, 5_000_000);
        uint64 exp = uint64(block.timestamp + 1 hours);
        bytes32 att = keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(), d, seller, exp));
        VoucherClaim[] memory rows = new VoucherClaim[](1);
        rows[0] = VoucherClaim({voucher: Voucher({channel: cfg, maxClaimableAmount: 5_000_000}),
            signature: abi.encode(_sig(agentPk,d), _sig(oraclePk,att), seller, exp), totalClaimed: 5_000_000});

        vm.prank(seller); esc.claim(rows);           // seller front-runs the revoke
        vm.prank(oracle); cs.revoke(seller, 1); // too late
        esc.settle(seller, USDC);
        assertEq(IERC20(USDC).balanceOf(seller), 5_000_000, "claimed funds are final; revocation cannot claw back");
        console.log("LIMITATION confirmed: claim-before-revoke is irreversible");
    }
}

/// The deposit collector moves a wallet's approved USDC. Only Coinbase's escrow may make it
/// move, and only into the escrow: anyone else calling it must get nothing.
contract CollectorAccessTest is Test {
    address constant ESCROW = 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003;
    address constant USDC   = 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913;

    function test_StrangerCannotDrainTheAllowance() public {
        vm.createSelectFork(vm.envString("BASE_RPC"));
        address owner = address(0x0117E5);
        address attacker = address(0xBADBAD);
        CountersignCollector collector = new CountersignCollector(ESCROW);
        Countersign cs = new Countersign(owner, ESCROW, address(0xA11CE), address(0x0DACE));
        deal(USDC, address(cs), 10_000_000);
        vm.prank(owner);
        cs.approveToken(USDC, address(collector), 10_000_000);

        vm.prank(attacker);
        vm.expectRevert();
        collector.collect(address(cs), USDC, 10_000_000, bytes32(0), "");
        assertEq(IERC20(USDC).balanceOf(attacker), 0, "the attacker walked away with the wallet's USDC");
        assertEq(IERC20(USDC).balanceOf(address(cs)), 10_000_000);
    }
}
