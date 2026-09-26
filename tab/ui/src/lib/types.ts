// The shapes the Tab server returns. Amounts are atomic USDC (6 decimals); serde writes u128 as
// a JSON number, so they arrive as numbers (or strings, if Tab chooses to stringify them).

export type Atomic = number | string;

/** One payment channel the countersigner has signed for (countersigner `Session`). */
export interface Session {
  channel_id: string;
  wallet: string;
  seller: string;
  token: string;
  chain_id: number;
  /** Highest ceiling countersigned so far: what the seller could claim right now. */
  ceiling: Atomic;
  vouchers: number;
  /** When the newest attestation lapses. After it, nothing signed here is claimable. */
  expiry: number;
  opened_at: number;
  last_screened_at: number;
  last_score: number;
  revoked: boolean;
  revoked_reason: string | null;
  /** Optional, from Tab: a human name for the seller (Bazaar listing). */
  label?: string;
}

/** An approval as the countersigner shows it: no device code, no subject. */
export interface Approval {
  id: string;
  purpose: "payment" | "restore" | string;
  status: "pending" | "granted" | "denied" | "expired" | string;
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

/** GET /api/overview */
export interface Overview {
  wallet: string;
  owner?: string;
  chainId: number;
  /** Payments are running against a local fork, not the live chain. */
  fork: boolean;
  /** Tab has no admin token configured: the dashboard can watch but not act. */
  readOnly: boolean;
  mcpUrl: string;
  /** Short handle for the World ID human this wallet is bound to, if any. */
  human: string | null;
  paused?: boolean;
  sessions: Session[];
  closedSellers: string[];
  approvals: Approval[];
}

/** GET /api/census -- live x402 batch-settlement channels on Base mainnet. */
export interface Census {
  windowDays: number;
  channels: number;
  payers: number;
  /** USDC currently held in the escrow across those channels. */
  inEscrow?: Atomic;
  /** Channels whose payer is a single hot key (EOA). */
  singleKey: number;
  /** Channels whose payer validates through EIP-1271 policy. */
  withPolicy: number;
  updatedAt: number;
}

export type FeedKind =
  | "screened"
  | "countersigned"
  | "approval_requested"
  | "approval_granted"
  | "approval_denied"
  | "human_bound"
  | "revoked"
  | "restored"
  // Tab's own side of the story
  | "tab_opened"
  | "purchase"
  | "claimed"
  | "claim_rejected"
  | (string & {});

/** One entry in Tab's merged feed (GET /api/feed, SSE /api/feed/stream). */
export interface FeedItem {
  id: number;
  at: number;
  source: "brain" | "tab";
  kind: FeedKind;
  [field: string]: unknown;
}
