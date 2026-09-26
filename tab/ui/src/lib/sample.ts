// Shown only when the Tab server can't be reached, and always labelled as sample data.
// The census figures are the ones measured on Base mainnet on 26 Sep 2026 (see the README).

import type { Approval, Census, FeedItem, Overview } from "./types";
import { now } from "./format";

const WALLET = "0x7a3c9e1f4b2d8a6c5e0f1d3b9a7c2e4f6d8b0a1c";
const HYPER = "0x4f2a91c07d3e5b8a6f1c2d9e0b7a3c5f8e1d4b6a";
const ONESOURCE = "0x9b1e7c3a5d2f8e0b4a6c1d7f3e9a5b2c8d0f6e4a";
const DEMO = "0xc81d3f5a7e9b2c4d6f8a0e1b3c5d7f9a2b4c6e8d";

export const SAMPLE_CENSUS: Census = {
  windowDays: 14,
  channels: 379,
  payers: 61,
  singleKey: 379,
  withPolicy: 0,
  updatedAt: 0,
};

export function sampleOverview(): Overview {
  const t = now();
  return {
    wallet: WALLET,
    owner: "0x2e8b4d6f1a3c5e7b9d0f2a4c6e8b1d3f5a7c9e0b",
    chainId: 8453,
    fork: false,
    readOnly: true,
    mcpUrl: `${location.origin}/mcp`,
    human: "Orb-verified",
    paused: false,
    sessions: [
      {
        channel_id: "0x51c0…",
        wallet: WALLET,
        seller: HYPER,
        label: "hyperextend",
        token: "USDC",
        chain_id: 8453,
        ceiling: 340_000,
        vouchers: 17,
        expiry: t + 94,
        opened_at: t - 2400,
        last_screened_at: t - 12,
        last_score: 4,
        revoked: false,
        revoked_reason: null,
      },
      {
        channel_id: "0x88ae…",
        wallet: WALLET,
        seller: ONESOURCE,
        label: "onesource",
        token: "USDC",
        chain_id: 8453,
        ceiling: 125_000,
        vouchers: 5,
        expiry: t + 41,
        opened_at: t - 900,
        last_screened_at: t - 8,
        last_score: 11,
        revoked: false,
        revoked_reason: null,
      },
      {
        channel_id: "0x2f07…",
        wallet: WALLET,
        seller: DEMO,
        label: "market-data",
        token: "USDC",
        chain_id: 8453,
        ceiling: 20_000,
        vouchers: 2,
        expiry: t + 63,
        opened_at: t - 600,
        last_screened_at: t - 30,
        last_score: 82,
        revoked: true,
        revoked_reason: "wallet_drainer",
      },
    ],
    closedSellers: [DEMO],
    approvals: [],
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
    ...data,
  });
  return [
    item(2400, "tab", "tab_opened", { seller: HYPER, deposit: 1_000_000 }),
    item(2398, "brain", "screened", { seller: HYPER, verdict: "pay", reason: "no risk traits", toxic_score: 4, requested: 20_000 }),
    item(2397, "brain", "countersigned", { seller: HYPER, ceiling: 20_000, expiry: t - 2277 }),
    item(900, "brain", "screened", { seller: ONESOURCE, verdict: "pay", reason: "no risk traits", toxic_score: 11, requested: 25_000 }),
    item(610, "brain", "screened", {
      seller: "0x098b716b8aaf21512996dc57eb0615e2383e2f96",
      verdict: "refuse",
      reason: "known scammer: linked to a reported drainer cluster",
      toxic_score: 97,
      requested: 50_000,
    }),
    item(420, "brain", "screened", { seller: ONESOURCE, verdict: "ask", reason: "over the 0.10 USDC per-call limit", toxic_score: 11, requested: 150_000 }),
    item(419, "brain", "approval_requested", { approval_id: "apr_7Qk2", purpose: "payment", seller: ONESOURCE, amount: 150_000, reason: "over the 0.10 USDC per-call limit" }),
    item(371, "brain", "approval_granted", { approval_id: "apr_7Qk2", purpose: "payment", orb_verified: true }),
    item(300, "tab", "purchase", { seller: DEMO, price: 10_000, path: "/v1/data" }),
    item(240, "tab", "purchase", { seller: DEMO, price: 10_000, path: "/v1/data" }),
    item(30, "brain", "revoked", {
      seller: DEMO,
      reason: "wallet_drainer",
      reason_code: 4,
      by: "watcher",
      value_stopped: 20_000,
      vouchers_stopped: 2,
      score_before: 6,
      score_now: 82,
    }),
    item(12, "tab", "claim_rejected", { seller: DEMO, amount: 20_000, reason: "the payer revoked this seller after it was paid" }),
    item(6, "brain", "countersigned", { seller: HYPER, ceiling: 340_000, expiry: t + 94 }),
  ];
}

export function sampleApproval(id: string): Approval {
  const t = now();
  return {
    id,
    purpose: "payment",
    status: "pending",
    wallet: WALLET,
    seller: ONESOURCE,
    amount: 150_000,
    reason: "Over the 0.10 USDC per-call limit you set",
    userCode: "KQXT-MRWD",
    verificationUri: "https://id.worldcoin.org/device",
    createdAt: t - 20,
    expiresAt: t + 280,
  };
}
