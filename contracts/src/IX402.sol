// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

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

interface IX402BatchSettlement {
    function deposit(ChannelConfig calldata config, uint128 amount, address collector, bytes calldata collectorData) external;
    function claim(VoucherClaim[] calldata voucherClaims) external;
    function claimWithSignature(VoucherClaim[] calldata voucherClaims, bytes calldata authorizerSignature) external;
    function settle(address receiver, address token) external;
    function initiateWithdraw(ChannelConfig calldata config, uint128 amount) external;
    function finalizeWithdraw(ChannelConfig calldata config) external;
    function getChannelId(ChannelConfig calldata config) external view returns (bytes32);
    function getVoucherDigest(bytes32 channelId, uint128 maxClaimableAmount) external view returns (bytes32);
    function getClaimBatchDigest(VoucherClaim[] calldata voucherClaims) external view returns (bytes32);
    function channels(bytes32) external view returns (uint128 balance, uint128 totalClaimed);
    function pendingWithdrawals(bytes32) external view returns (uint128 amount, uint40 finalizeAfter);
}
interface IDepositCollector {
    function collect(address payer, address token, uint256 amount, bytes32 channelId, bytes calldata data) external;
}
interface IERC20 {
    function transferFrom(address f, address t, uint256 a) external returns (bool);
    function transfer(address t, uint256 a) external returns (bool);
    function approve(address s, uint256 a) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}
