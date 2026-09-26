// Shown only when the Tab server can't be reached, and always labelled as sample data.
// The census figures are the ones Tab measured on Base mainnet on 27 Sep 2026; the sellers are the
// real hyperextend and onesource payTo addresses, and the demo shop is ours.

import type { ApprovalPage, Census, FeedItem, Me } from "./types";
import { now } from "./format";

const WALLET = "0x7a3c9e1f4b2d8a6c5e0f1d3b9a7c2e4f6d8b0a1c";
const HYPER = "0x548fc289526ab2f0391d562a723cda64bbf1abc4";
const ONESOURCE = "0x52e29e0d2aa49bfbfc548c0a9f2196f4aa51f3ea";
const DEMO = "0xc81d3f5a7e9b2c4d6f8a0e1b3c5d7f9a2b4c6e8d";

export const SAMPLE_CENSUS: Census = {
  measuredAt: 1790451903,
  channels: 1928,
  payers: 204,
  sellers: 153,
  channelsLast14d: 591,
  escrowUsdc: 56_625_177,
  policyChannels: 3,
  countersignPayers: 1,
};

export function sampleMe(): Me {
  const t = now();
  return {
    account: { id: "sample", wallet: WALLET, human: "Orb-verified", createdAt: t - 3600, trial: 250_000, deployTx: null, fundTx: null },
    wallet: { usdc: 120_000, allowance: 1_000_000, paused: false, owner: "0x2e8b4d6f1a3c5e7b9d0f2a4c6e8b1d3f5a7c9e0b", riskOracle: "0xd2ba14d0bf9163a135270b562c8c7cba2685175d" },
    totals: { spent: 45_000, stoppable: 25_000, escrowed: 105_000, saved: 3_000 },
    tabs: [
      {
        channelId: "0x51c0",
        seller: HYPER,
        service: "hyperextend",
        price: 2_000,
        requests: 10,
        charged: 20_000,
        claimed: 0,
        stoppable: 20_000,
        saved: 0,
        escrowed: 50_000,
        deposited: 50_000,
        expiry: t + 94,
        withdrawing: false,
        status: "open",
        riskScore: 4,
        openTx: null,
      },
      {
        channelId: "0x88ae",
        seller: ONESOURCE,
        service: "onesource",
        price: 1_000,
        requests: 5,
        charged: 5_000,
        claimed: 0,
        stoppable: 5_000,
        saved: 0,
        escrowed: 50_000,
        deposited: 50_000,
        expiry: t + 41,
        withdrawing: false,
        status: "open",
        riskScore: 11,
        openTx: null,
      },
      {
        channelId: "0x2f07",
        seller: DEMO,
        service: "Tab demo shop",
        price: 1_000,
        requests: 5,
        charged: 5_000,
        claimed: 2_000,
        stoppable: 0,
        saved: 3_000,
        escrowed: 5_000,
        deposited: 50_000,
        expiry: t + 63,
        withdrawing: false,
        status: "closed",
        riskScore: 0,
        openTx: null,
      },
    ],
    approvals: [],
    receipts: [],
    mcpUrl: `${location.origin}/mcp`,
    network: { chainId: 8453, fork: false },
  };
}

export function sampleFeed(): FeedItem[] {
  const t = now();
  let id = 0;
  const item = (ago: number, source: FeedItem["source"], kind: string, data: Record<string, unknown>): FeedItem => ({
    id: ++id,
    at: t - ago,
    source,
    kind,
    wallet: WALLET,
    ...data,
  });
  return [
    item(2400, "tab", "tab_opened", { seller: HYPER, service: "hyperextend", deposit: 50_000 }),
    item(2398, "brain", "screened", { seller: HYPER, verdict: "pay", reason: "no adverse signals", toxic_score: 4, requested: 2_000 }),
    item(2397, "brain", "countersigned", { seller: HYPER, ceiling: 2_000, expiry: t - 2277 }),
    item(2396, "tab", "purchase", { seller: HYPER, service: "hyperextend", price: 2_000, url: "https://api.hyperextend.xyz/v1/candles/BTC/1m/latest" }),
    item(900, "brain", "screened", { seller: ONESOURCE, verdict: "pay", reason: "no adverse signals", toxic_score: 11, requested: 1_000 }),
    item(610, "brain", "screened", {
      seller: "0x098b716b8aaf21512996dc57eb0615e2383e2f96",
      verdict: "refuse",
      reason: "known scammer, sanctioned address",
      toxic_score: 100,
      requested: 1_000,
    }),
    item(420, "brain", "screened", { seller: ONESOURCE, verdict: "ask", reason: "over the 0.05 USDC this tab may spend on its own", toxic_score: 11, requested: 60_000 }),
    item(419, "brain", "approval_requested", { approval_id: "apr_7Qk2", purpose: "payment", seller: ONESOURCE, amount: 60_000, reason: "over the 0.05 USDC this tab may spend on its own" }),
    item(371, "brain", "approval_granted", { approval_id: "apr_7Qk2", purpose: "payment", orb_verified: true }),
    item(300, "tab", "purchase", { seller: DEMO, service: "Tab demo shop", price: 1_000, url: "/v1/data" }),
    item(240, "tab", "purchase", { seller: DEMO, service: "Tab demo shop", price: 1_000, url: "/v1/data" }),
    item(30, "brain", "revoked", { seller: DEMO, reason: "sold junk", reason_code: 99, by: "operator", value_stopped: 3_000, vouchers_stopped: 3 }),
    item(6, "brain", "countersigned", { seller: HYPER, ceiling: 20_000, expiry: t + 94 }),
  ];
}

export function sampleApproval(id: string): ApprovalPage {
  const t = now();
  return {
    approval: {
      id,
      purpose: "payment",
      status: "pending",
      wallet: WALLET,
      seller: ONESOURCE,
      amount: 60_000,
      reason: "Over the 0.05 USDC this tab may spend on its own",
      userCode: "KQXT-MRWD",
      verificationUri: "https://sandbox.auth.world.org/device",
      createdAt: t - 20,
      expiresAt: t + 280,
    },
    service: "onesource",
    worldUrl: "https://sandbox.auth.world.org/device",
  };
}
