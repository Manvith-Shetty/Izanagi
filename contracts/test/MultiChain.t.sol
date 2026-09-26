// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;
import {Test, console} from "forge-std/Test.sol";
import {Countersign, CountersignCollector} from "../src/Countersign.sol";
import {IX402BatchSettlement, IERC20, ChannelConfig, Voucher, VoucherClaim} from "../src/IX402.sol";

contract MultiChainTest is Test {
    address constant ESCROW = 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003;
    IX402BatchSettlement esc = IX402BatchSettlement(ESCROW);
    uint256 aPk = 0xA11CE; uint256 oPk = 0x0DACE; uint256 rPk = 0xBEEF;
    address owner = address(0x0117E5);

    function _sig(uint256 pk, bytes32 h) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk,h); return abi.encodePacked(r,s,v);
    }

    function _runOn(string memory rpc, address usdc, string memory label) internal {
        vm.createSelectFork(rpc);
        assertGt(ESCROW.code.length, 10000, "escrow deployed");
        address agent=vm.addr(aPk); address oracle=vm.addr(oPk); address rAuth=vm.addr(rPk);
        address seller=address(0x5E11E5);
        vm.etch(agent,""); vm.etch(oracle,""); vm.etch(rAuth,""); vm.etch(seller,"");
        CountersignCollector col = new CountersignCollector(ESCROW);
        Countersign cs = new Countersign(owner, ESCROW, agent, oracle);
        deal(usdc, address(cs), 20_000_000);
        ChannelConfig memory cfg = ChannelConfig({payer:address(cs), payerAuthorizer:address(0),
            receiver:seller, receiverAuthorizer:rAuth, token:usdc, withdrawDelay:15 minutes, salt:bytes32(uint256(3))});
        vm.startPrank(owner);
        cs.approveToken(usdc,address(col),type(uint256).max);
        cs.openChannel(cfg,5_000_000,address(col));
        vm.stopPrank();
        bytes32 cid=esc.getChannelId(cfg);
        bytes32 d=esc.getVoucherDigest(cid,2_000_000);
        uint64 exp=uint64(block.timestamp+1 hours);
        bytes32 att=keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(),d,seller,exp));
        VoucherClaim[] memory rows=new VoucherClaim[](1);
        rows[0]=VoucherClaim({voucher:Voucher({channel:cfg,maxClaimableAmount:2_000_000}),
            signature:abi.encode(_sig(aPk,d),_sig(oPk,att),seller,exp), totalClaimed:2_000_000});
        esc.claimWithSignature(rows,_sig(rPk,esc.getClaimBatchDigest(rows)));
        esc.settle(seller,usdc);
        assertEq(IERC20(usdc).balanceOf(seller),2_000_000);
        console.log(label, "OK - escrow bytes:", ESCROW.code.length);

        // revocation works on this chain too
        vm.prank(oracle); cs.revoke(seller, 1);
        bytes32 d2=esc.getVoucherDigest(cid,4_000_000);
        bytes32 att2=keccak256(abi.encode(cs.ATTESTATION_TYPEHASH(),d2,seller,exp));
        rows[0]=VoucherClaim({voucher:Voucher({channel:cfg,maxClaimableAmount:4_000_000}),
            signature:abi.encode(_sig(aPk,d2),_sig(oPk,att2),seller,exp), totalClaimed:4_000_000});
        bytes memory bs=_sig(rPk,esc.getClaimBatchDigest(rows));
        vm.expectRevert();
        esc.claimWithSignature(rows,bs);
        console.log(label, "revocation enforced");
    }

    function test_Optimism() public { _runOn("https://mainnet.optimism.io", 0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85, "OPTIMISM"); }
    function test_Arbitrum() public { _runOn("https://arb1.arbitrum.io/rpc", 0xaf88d065e77c8cC2239327C5EDb3A432268e5831, "ARBITRUM"); }
}
