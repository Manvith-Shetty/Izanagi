// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {Countersign, CountersignCollector} from "../src/Countersign.sol";
import {IX402BatchSettlement, IERC20, ChannelConfig, Voucher, VoucherClaim} from "../src/IX402.sol";

/// The Tab lifecycle against the real escrow: a wallet works from the transaction that creates
/// it, the agent opens tabs from its own funds with no allowance left standing, a stranger
/// cannot route those funds into a channel they authorise, and only the owner moves ownership.
contract TabFlowTest is Test {
    address constant ESCROW = 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003;
    address constant USDC   = 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913;

    IX402BatchSettlement esc = IX402BatchSettlement(ESCROW);
    Countersign cs;
    CountersignCollector col;

    address operator = address(0x0117E5); // the person's MetaMask: they deploy and own the wallet
    address person   = makeAddr("another account of theirs");
    address agent    = address(0xA11CE);
    address oracle   = address(0x0DACE);
    address seller   = address(0x5E11E5);
    uint256 evilPk   = 0xBAD0BAD;
    address evil;

    function setUp() public {
        vm.createSelectFork(vm.envString("BASE_RPC"));
        evil = vm.addr(evilPk);
        vm.etch(evil, ""); vm.etch(seller, ""); vm.etch(person, "");
        col = new CountersignCollector(ESCROW);
        cs = new Countersign(operator, ESCROW, agent, oracle, address(col));
        deal(USDC, address(cs), 10_000_000);
    }

    function test_AWalletHasItsCollectorFromTheStart() public view {
        assertEq(cs.collector(), address(col));
        assertEq(cs.owner(), operator);
    }

    function _cfg(address authorizer, uint40 delay, uint256 salt) internal view returns (ChannelConfig memory) {
        return ChannelConfig({payer: address(cs), payerAuthorizer: authorizer, receiver: seller,
            receiverAuthorizer: seller, token: USDC, withdrawDelay: delay, salt: bytes32(salt)});
    }

    /* ------------------------------ the agent opens tabs ------------------------------ */

    function test_AgentOpensATabAndLeavesNoAllowance() public {
        ChannelConfig memory c = _cfg(address(0), 1 days, 1);
        vm.prank(agent);
        cs.openTab(c, 2_000_000);
        (uint128 bal,) = esc.channels(esc.getChannelId(c));
        assertEq(bal, 2_000_000, "the tab holds the deposit");
        assertEq(IERC20(USDC).balanceOf(address(cs)), 8_000_000);
        assertEq(IERC20(USDC).allowance(address(cs), address(col)), 0, "no allowance left standing");
    }

    function test_AgentCannotOpenAChannelSomeoneElseAuthorises() public {
        vm.prank(agent);
        vm.expectRevert(Countersign.BadChannel.selector);
        cs.openTab(_cfg(evil, 1 days, 2), 2_000_000);
    }

    function test_AgentCannotLockFundsForLong() public {
        vm.prank(agent);
        vm.expectRevert(Countersign.BadChannel.selector);
        cs.openTab(_cfg(address(0), 30 days, 3), 2_000_000);
    }

    function test_StrangerCannotOpenATab() public {
        vm.prank(evil);
        vm.expectRevert(Countersign.NotAgent.selector);
        cs.openTab(_cfg(address(0), 1 days, 4), 2_000_000);
    }

    function test_NoTabWithoutACollector() public {
        vm.prank(operator);
        cs.setCollector(address(0));
        vm.prank(agent);
        vm.expectRevert(Countersign.NoCollector.selector);
        cs.openTab(_cfg(address(0), 1 days, 5), 2_000_000);
    }

    /* --------------------- a standing allowance cannot be hijacked --------------------- */

    /// The escrow's deposit is open to anyone. With an allowance standing, a stranger used to be
    /// able to open a channel funded by the wallet, authorised by themselves, and claim it all.
    function test_StrangerCannotRouteTheAllowanceIntoTheirOwnChannel() public {
        vm.prank(operator);
        cs.approveToken(USDC, address(col), type(uint256).max);
        ChannelConfig memory c = ChannelConfig({payer: address(cs), payerAuthorizer: evil, receiver: evil,
            receiverAuthorizer: evil, token: USDC, withdrawDelay: 15 minutes, salt: bytes32(uint256(66))});

        vm.startPrank(evil);
        vm.expectRevert();
        esc.deposit(c, 10_000_000, address(col), abi.encode(c));
        vm.expectRevert();
        esc.deposit(c, 10_000_000, address(col), "");
        // lying about the config: a gated one in the data, the stranger's channel for real
        ChannelConfig memory decoy = _cfg(address(0), 1 days, 67);
        vm.expectRevert();
        esc.deposit(c, 10_000_000, address(col), abi.encode(decoy));
        vm.stopPrank();

        assertEq(IERC20(USDC).balanceOf(address(cs)), 10_000_000, "the wallet kept its funds");
    }

    /* ----------------------------------- the handover ----------------------------------- */

    function test_HandoverMovesEveryOwnerPower() public {
        vm.prank(operator);
        cs.transferOwnership(person);
        assertEq(cs.owner(), person);

        vm.startPrank(operator);
        vm.expectRevert(Countersign.NotOwner.selector);
        cs.sweep(USDC, operator, 1);
        vm.expectRevert(Countersign.NotOwner.selector);
        cs.transferOwnership(operator);
        vm.expectRevert(Countersign.NotOwner.selector);
        cs.setAgent(operator);
        vm.stopPrank();

        vm.prank(person);
        cs.sweep(USDC, person, 10_000_000);
        assertEq(IERC20(USDC).balanceOf(person), 10_000_000, "the new owner takes the money out");
    }

    function test_NewOwnerRecoversATabTheAgentOpened() public {
        ChannelConfig memory c = _cfg(address(0), 1 days, 7);
        vm.prank(agent);
        cs.openTab(c, 3_000_000);
        vm.prank(operator);
        cs.transferOwnership(person);

        vm.startPrank(person);
        cs.initiateWithdraw(c, 3_000_000);
        vm.warp(block.timestamp + 1 days + 1);
        cs.finalizeWithdraw(c);
        cs.sweep(USDC, person, 10_000_000);
        vm.stopPrank();
        assertEq(IERC20(USDC).balanceOf(person), 10_000_000);
    }

    function test_AgentStillOpensTabsAfterTheHandover() public {
        vm.prank(operator);
        cs.transferOwnership(person);
        vm.prank(agent);
        cs.openTab(_cfg(address(0), 1 days, 8), 1_000_000);
        assertEq(IERC20(USDC).balanceOf(address(cs)), 9_000_000);
    }

    function test_HandoverRefusesNobody() public {
        vm.prank(operator);
        vm.expectRevert(Countersign.ZeroOwner.selector);
        cs.transferOwnership(address(0));
    }

    function test_OnlyTheOwnerHandsOver() public {
        vm.prank(agent);
        vm.expectRevert(Countersign.NotOwner.selector);
        cs.transferOwnership(agent);
        vm.prank(oracle);
        vm.expectRevert(Countersign.NotOwner.selector);
        cs.transferOwnership(oracle);
    }
}
