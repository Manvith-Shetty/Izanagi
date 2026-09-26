// Tab's HTTP API, as this site uses it. Same-origin in production (Tab serves this build);
// proxied to TAB_DEV_PROXY by `npm run dev`. The session is an HttpOnly cookie Tab sets when a
// person signs in with World ID.
//
//   GET  /api/network                  live x402 channels on Base mainnet
//   POST /api/signup                   start signing in with World ID
//   GET  /api/signup/{id}              where that has got to
//   POST /api/logout
//   GET  /api/me                       this person's wallet, tabs and approvals   (signed in)
//   GET  /api/stream                   their feed: history, then live (SSE)       (signed in)
//   GET  /api/approvals/{id}           one approval, for the approval page
//   POST /api/tabs/close   {seller, reason?}   stop paying a seller, now           (signed in)
//   POST /api/tabs/reopen  {seller}            ask the human to allow it again     (signed in)
//
// When Tab can't be reached at all the site falls back to labelled sample data so it can still
// be shown. A Tab that answers "sign in first" is not a fallback: that shows the sign-in.

import type { ApprovalPage, Atomic, Census, FeedItem, Me, Stage } from "./types";
import { SAMPLE_CENSUS, sampleApproval, sampleMe } from "./sample";

export interface Loaded<T> {
  data: T;
  /** True when Tab didn't answer and `data` is sample data. */
  sample: boolean;
}

/** Tab answered, but not with JSON: it is not running behind /api (the proxy's own page came back). */
class Unreachable extends Error {}

async function get<T>(path: string): Promise<{ status: number; body: T }> {
  let res: Response;
  try {
    res = await fetch(path, { credentials: "same-origin", headers: { accept: "application/json" } });
  } catch {
    throw new Unreachable(path);
  }
  const type = res.headers.get("content-type") ?? "";
  if (!type.includes("json") || res.status >= 502) throw new Unreachable(`${path} answered ${res.status}`);
  return { status: res.status, body: (await res.json()) as T };
}

export type MeState = { kind: "signed_in"; me: Me } | { kind: "signed_out" } | { kind: "sample"; me: Me };

export async function loadMe(): Promise<MeState> {
  try {
    const r = await get<Me>("/api/me");
    if (r.status === 401) return { kind: "signed_out" };
    if (r.status >= 400) throw new Unreachable(`/api/me answered ${r.status}`);
    return { kind: "signed_in", me: r.body };
  } catch {
    return { kind: "sample", me: sampleMe() };
  }
}

export async function loadCensus(): Promise<Loaded<Census>> {
  try {
    const r = await get<{ census: Census }>("/api/network");
    if (r.status >= 400) throw new Unreachable(`/api/network answered ${r.status}`);
    return { data: r.body.census, sample: false };
  } catch {
    return { data: SAMPLE_CENSUS, sample: true };
  }
}

export class NotFound extends Error {}

export async function loadApproval(id: string): Promise<Loaded<ApprovalPage>> {
  let r: { status: number; body: ApprovalPage };
  try {
    r = await get<ApprovalPage>(`/api/approvals/${encodeURIComponent(id)}`);
  } catch {
    return { data: sampleApproval(id), sample: true };
  }
  if (r.status === 404) throw new NotFound("This approval doesn't exist, or has been cleared.");
  if (r.status >= 400) throw new Error(`Tab answered ${r.status}`);
  return { data: r.body, sample: false };
}

/* ---------------------------------- actions ---------------------------------- */

export class ActionError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}

async function post<T>(path: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ActionError("Tab isn't answering. Check that the Tab server is running.", 0);
  }
  const json = (await res.json().catch(() => ({}))) as Record<string, unknown>;
  if (res.status === 401) throw new ActionError("You're signed out. Sign in with World ID again.", 401);
  if (!res.ok) throw new ActionError(String(json.error ?? json.reason ?? `Tab answered ${res.status}`), res.status);
  return json as T;
}

export const signup = () => post<{ id: string; stage: Stage }>("/api/signup");
export const logout = () => post<{ ok: boolean }>("/api/logout");

export async function signupPoll(id: string): Promise<{ id: string; stage: Stage }> {
  const r = await get<{ id: string; stage: Stage; error?: string }>(`/api/signup/${encodeURIComponent(id)}`).catch(() => {
    throw new ActionError("Tab isn't answering. Check that the Tab server is running.", 0);
  });
  if (r.status >= 400) throw new ActionError(r.body.error ?? `Tab answered ${r.status}`, r.status);
  return r.body;
}

export const closeTab = (seller: string, reason?: string) =>
  post<{ value_stopped: Atomic; vouchers_stopped: number; tx: string | null }>("/api/tabs/close", { seller, reason });
export const reopenTab = (seller: string) => post<{ id: string; approvalUrl: string }>("/api/tabs/reopen", { seller });

/* ------------------------------------ live ------------------------------------ */

/** Follow this person's feed: history first, then live. Returns a function that stops following. */
export function followFeed(onItem: (i: FeedItem) => void, onState: (live: boolean) => void): () => void {
  let es: EventSource | null = null;
  try {
    es = new EventSource("/api/stream", { withCredentials: true });
  } catch {
    onState(false);
    return () => {};
  }
  es.addEventListener("open", () => onState(true));
  es.addEventListener("error", () => onState(false));
  es.addEventListener("item", (e) => {
    try {
      onItem(JSON.parse((e as MessageEvent).data) as FeedItem);
    } catch {
      /* a malformed frame is skipped, not fatal */
    }
  });
  return () => es?.close();
}
