// The shapes the Tab server returns (tab/src/api.rs, tab/src/app.rs). Amounts are atomic USDC
// (6 decimals); serde writes u128 as a JSON number.

export type Atomic = number | string;

/** One tab: a payment channel from this person's wallet to one seller (`app.rs` overview). */
export interface Tab {
  channelId: string;
  seller: string;
  /** The seller's name, when Tab knows it. */
  service: string | null;
  price: Atomic;
  requests: number;
  /** Everything the agent has signed for on this tab. */
  charged: Atomic;
  /** What the seller has already cashed in. */
  claimed: Atomic;
  /** Signed, not yet claimed, and still claimable: what closing the tab would stop. */
  stoppable: Atomic;
  /** Signed but never cashed in, on a tab now closed: money the person kept. */
  saved: Atomic;
  escrowed: Atomic;
  deposited: Atomic;
  /** When the newest countersignature lapses. After it, nothing signed here is claimable. */
  expiry: number;
  withdrawing: boolean;
  status: "open" | "closed" | "lapsed";
  /** Intercepta's latest toxic score for the seller. */
  riskScore: number | null;
  openTx: string | null;
}

/** An approval as the countersigner shows it: no subject, nothing that identifies the person. */
export interface Approval {
  id: string;
  purpose: "payment" | "restore" | "enroll" | (string & {});
  status: "pending" | "approved" | "used" | "denied" | "expired" | (string & {});
  wallet: string;
  seller: string;
  amount: Atomic;
  reason: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete?: string;
  createdAt: number;
  expiresAt: number;
  deniedReason?: string;
  orbVerified?: boolean;
  tx?: string;
}

/** GET /api/approvals/{id} */
export interface ApprovalPage {
  approval: Approval;
  /** The seller's name, when Tab knows it. */
  service: string | null;
  /** The World App link that approves it. */
  worldUrl: string;
}

export interface Receipt {
  at: number;
  seller: string;
  service: string | null;
  url: string;
  price: Atomic;
  cumulative: Atomic;
  verdict: string;
  toxicScore: number;
}

/** GET /api/me -- the signed-in person's wallet and tabs. */
export interface Me {
  account: { id: string; wallet: string; human: string; createdAt: number; trial: Atomic; deployTx: string | null; fundTx: string | null };
  wallet: { usdc: Atomic; allowance: Atomic; paused: boolean; owner: string; riskOracle: string } | null;
  totals: { spent: Atomic; stoppable: Atomic; escrowed: Atomic; saved: Atomic };
  tabs: Tab[];
  approvals: Approval[];
  receipts: Receipt[];
  mcpUrl: string;
  network: { chainId: number; fork: boolean };
}

/** GET /api/network -> census: live x402 batch-settlement channels on Base mainnet. */
export interface Census {
  /** Unix seconds; 0 until the first measurement lands. */
  measuredAt: number;
  channels: number;
  payers: number;
  sellers: number;
  channelsLast14d: number;
  /** USDC the escrow holds right now, atomic. */
  escrowUsdc: Atomic;
  /** Channels whose payer is checked by EIP-1271 at claim time. */
  policyChannels: number;
  /** Of those, payers that are Countersign wallets: they can stop a payment after it is signed. */
  countersignPayers: number;
}

/** Where a signup has got to (POST /api/signup, GET /api/signup/{id}). */
export type Stage =
  | { stage: "verifying"; user_code: string; world_url: string; expires_at: number }
  | { stage: "creating"; step: string }
  | { stage: "ready"; account: string; returning: boolean }
  | { stage: "failed"; reason: string };

export type FeedKind =
  // the countersigner's decisions
  | "screened"
  | "countersigned"
  | "approval_requested"
  | "approval_granted"
  | "approval_denied"
  | "human_bound"
  | "revoked"
  | "restored"
  // what Tab did about them
  | "account_created"
  | "tab_opened"
  | "tab_topped_up"
  | "purchase"
  | "purchase_refused"
  | "approval_needed"
  | "seller_claimed"
  | (string & {});

/** One entry in this person's feed (SSE /api/stream, `event: item`). */
export interface FeedItem {
  id: number;
  at: number;
  source: "brain" | "tab";
  kind: FeedKind;
  [field: string]: unknown;
}
