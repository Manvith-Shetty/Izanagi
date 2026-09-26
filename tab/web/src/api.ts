// Talking to the Tab server. Everything is same-origin; the session is an HttpOnly cookie.

export type Stage =
  | { stage: "verifying"; user_code: string; world_url: string; expires_at: number }
  | { stage: "creating"; step: string }
  | { stage: "ready"; account: string; returning: boolean }
  | { stage: "failed"; reason: string };

export interface Tab {
  channelId: string;
  seller: string;
  service: string | null;
  price: number;
  requests: number;
  charged: number;
  claimed: number;
  stoppable: number;
  saved: number;
  escrowed: number;
  deposited: number;
  expiry: number;
  withdrawing: boolean;
  status: "open" | "closed" | "lapsed";
  riskScore: number | null;
  openTx: string | null;
}

export interface ReceiptLine {
  at: number;
  seller: string;
  service: string | null;
  url: string;
  price: number;
  cumulative: number;
  verdict: string;
  toxicScore: number;
}

export interface ApprovalView {
  id: string;
  purpose: "payment" | "restore" | "enroll";
  status: "pending" | "approved" | "denied" | "expired" | "used";
  wallet: string;
  seller: string;
  amount: number;
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

export interface Me {
  account: { id: string; wallet: string; human: string; createdAt: number; trial: number; deployTx: string | null; fundTx: string | null };
  wallet: { usdc: number; allowance: number; paused: boolean; owner: string; riskOracle: string } | null;
  totals: { spent: number; stoppable: number; escrowed: number; saved: number };
  tabs: Tab[];
  approvals: ApprovalView[];
  receipts: ReceiptLine[];
  mcpUrl: string;
  network: { chainId: number; fork: boolean };
}

export interface Listing {
  url: string;
  name: string;
  description: string;
  price: number;
  seller: string;
  source: "curated" | "bazaar";
  proven?: string;
  demo: boolean;
}

export interface Census {
  measuredAt: number;
  channels: number;
  payers: number;
  sellers: number;
  channelsLast14d: number;
  escrowUsdc: number;
  policyChannels: number;
  countersignPayers: number;
}

export type Outcome =
  | ({ outcome: "paid" } & {
      url: string;
      seller: string;
      service: string | null;
      price: number;
      cumulative: number;
      stoppable: number;
      verdict: string;
      reason: string;
      toxicScore: number;
      status: number;
      data: unknown;
      opened?: { tx: string; deposit: number };
    })
  | ({ outcome: "needs_approval" } & {
      approvalId: string;
      approvalUrl: string;
      worldUrl: string;
      userCode: string;
      expiresIn: number;
      service: string | null;
      limit: number;
      reason: string;
    })
  | { outcome: "refused"; verdict: string; reason: string; toxicScore: number }
  | { outcome: "unpayable"; reason: string }
  | { outcome: "free"; status: number; body: string }
  | { outcome: "denied"; approval_id: string; reason: string }
  | { outcome: "still_pending"; approval_id: string; approval_url: string; user_code: string };

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "same-origin",
    ...init,
    headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok && res.status !== 202) throw new ApiError(res.status, body.error ?? `request failed (${res.status})`);
  return body as T;
}

export const api = {
  network: () => call<{ census: Census; accounts: number }>("/api/network"),
  catalog: () => call<{ listings: Listing[] }>("/api/catalog"),
  health: () => call<{ trialsLeft: number; fork: boolean; chainId: number }>("/api/health"),
  signup: () => call<{ id: string; stage: Stage }>("/api/signup", { method: "POST" }),
  signupPoll: (id: string) => call<{ id: string; stage: Stage }>(`/api/signup/${id}`),
  me: () => call<Me>("/api/me"),
  logout: () => call<{ ok: boolean }>("/api/logout", { method: "POST" }),
  buy: (url: string) => call<Outcome>("/api/buy", { method: "POST", body: JSON.stringify({ url }) }),
  wait: (id: string) => call<Outcome>(`/api/approvals/${id}/wait`, { method: "POST" }),
  approval: (id: string) => call<{ approval: ApprovalView; service: string | null; worldUrl: string }>(`/api/approvals/${id}`),
  close: (seller: string, reason?: string) =>
    call<{ value_stopped: number; vouchers_stopped: number; tx: string | null }>("/api/tabs/close", {
      method: "POST",
      body: JSON.stringify({ seller, reason }),
    }),
  reopen: (seller: string) =>
    call<ApprovalView & { approvalUrl: string }>("/api/tabs/reopen", { method: "POST", body: JSON.stringify({ seller }) }),
};

/* --------------------------------------------------------------- formatting */

/** Atomic USDC (6 decimals) as money: $0.002, $0.20, $1.00. Cents always shown; more
 *  decimals only when they carry a price smaller than a cent. `round` gives plain cents. */
export function usd(atomic: number, round = false): string {
  const whole = Math.floor(atomic / 1_000_000);
  const frac = atomic % 1_000_000;
  if (round) return `$${(atomic / 1_000_000).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  const digits = String(frac).padStart(6, "0").replace(/0+$/, "");
  return `$${whole.toLocaleString()}.${digits.padEnd(2, "0")}`;
}

export function short(addr: string): string {
  return addr.length > 12 ? `${addr.slice(0, 6)}…${addr.slice(-4)}` : addr;
}

export function scan(kind: "tx" | "address", hash: string, fork: boolean): string | null {
  return fork ? null : `https://basescan.org/${kind}/${hash}`;
}

export function clock(unix: number): string {
  return new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** "4m 10s", "2h 5m", "3d". */
export function left(secs: number): string {
  if (secs <= 0) return "0s";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (d > 0) return `${d}d${h ? ` ${h}h` : ""}`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}
