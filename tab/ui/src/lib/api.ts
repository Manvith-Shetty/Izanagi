// Tab's HTTP API, as this site uses it. Same-origin in production (Tab serves this build);
// proxied to TAB_DEV_PROXY by `npm run dev`.
//
//   GET  /api/overview                 the wallet, its tabs, pending approvals
//   GET  /api/census                   live x402 channels on Base mainnet
//   GET  /api/feed?after=N             the merged feed since N
//   GET  /api/feed/stream              the same, live (SSE, `event: item`)
//   GET  /api/approvals/{id}           one approval, for the approval page
//   POST /api/tabs/close   {seller, reason?}   stop paying a seller, now   (Bearer TAB_ADMIN_TOKEN)
//   POST /api/tabs/reopen  {seller}            ask the human to allow it again (Bearer TAB_ADMIN_TOKEN)
//
// Reads never need a token. When Tab can't be reached the site falls back to labelled
// sample data so it can still be shown; actions never fall back.

import type { Approval, Census, FeedItem, Overview } from "./types";
import { SAMPLE_CENSUS, sampleApproval, sampleFeed, sampleOverview } from "./sample";

export interface Loaded<T> {
  data: T;
  /** True when Tab didn't answer and `data` is sample data. */
  sample: boolean;
}

async function get<T>(path: string): Promise<T> {
  const res = await fetch(path, { headers: { accept: "application/json" } });
  const type = res.headers.get("content-type") ?? "";
  if (!res.ok || !type.includes("json")) throw new Error(`${path} answered ${res.status}`);
  return res.json() as Promise<T>;
}

async function orSample<T>(path: string, sample: () => T): Promise<Loaded<T>> {
  try {
    return { data: await get<T>(path), sample: false };
  } catch {
    return { data: sample(), sample: true };
  }
}

export const loadOverview = () => orSample<Overview>("/api/overview", sampleOverview);
export const loadCensus = () => orSample<Census>("/api/census", () => SAMPLE_CENSUS);
export const loadFeed = () =>
  orSample<FeedItem[]>("/api/feed?after=0", sampleFeed).then(async (r) => {
    // the endpoint wraps items: { items: [...] }
    const d = r.data as unknown as { items?: FeedItem[] } | FeedItem[];
    return { ...r, data: Array.isArray(d) ? d : (d.items ?? []) };
  });
export const loadApproval = (id: string) => orSample<Approval>(`/api/approvals/${encodeURIComponent(id)}`, () => sampleApproval(id));

/* -------------------------------- admin token -------------------------------- */

const TOKEN_KEY = "izanagi.admin";

export function adminToken(): string | null {
  try {
    return sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setAdminToken(t: string | null) {
  try {
    if (t) sessionStorage.setItem(TOKEN_KEY, t);
    else sessionStorage.removeItem(TOKEN_KEY);
  } catch {
    /* private mode: the token lives only as long as the page */
  }
}

export class ActionError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const token = adminToken();
  let res: Response;
  try {
    res = await fetch(path, {
      method: "POST",
      headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
      body: JSON.stringify(body),
    });
  } catch {
    throw new ActionError("Tab isn't answering. Check that the Tab server is running.", 0);
  }
  const json = (await res.json().catch(() => ({}))) as Record<string, unknown>;
  if (res.status === 401 || res.status === 403) throw new ActionError("Tab refused the admin token. Enter it again.", res.status);
  if (!res.ok) throw new ActionError(String(json.error ?? json.reason ?? `Tab answered ${res.status}`), res.status);
  return json as T;
}

export const closeTab = (seller: string, reason?: string) => post<FeedItem>("/api/tabs/close", { seller, reason });
export const reopenTab = (seller: string) => post<{ approvalId: string; approvalUrl: string }>("/api/tabs/reopen", { seller });

/* ------------------------------------ live ------------------------------------ */

/** Follow the merged feed. Returns a function that stops following. */
export function followFeed(onItem: (i: FeedItem) => void, onState: (live: boolean) => void): () => void {
  let es: EventSource | null = null;
  try {
    es = new EventSource("/api/feed/stream");
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
