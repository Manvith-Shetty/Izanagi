import type { Atomic } from "./types";

const DECIMALS = 1_000_000;

export function usdc(v: Atomic | undefined, digits = 2): string {
  const n = Number(v ?? 0) / DECIMALS;
  return n.toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: Math.max(digits, 6) });
}

/** Trims trailing zeros past two places: 0.340000 -> 0.34, 0.012500 -> 0.0125. */
export function usdcShort(v: Atomic | undefined): string {
  const n = Number(v ?? 0) / DECIMALS;
  const s = n.toFixed(6).replace(/0+$/, "");
  const [whole, frac = ""] = s.split(".");
  return `${Number(whole).toLocaleString("en-US")}.${frac.padEnd(2, "0")}`;
}

export function short(addr: string | undefined, head = 6, tail = 4): string {
  if (!addr) return "";
  return addr.length > head + tail + 1 ? `${addr.slice(0, head)}…${addr.slice(-tail)}` : addr;
}

export function now(): number {
  return Math.floor(Date.now() / 1000);
}

/** 83 -> "1:23", 3725 -> "1:02:05" */
export function clock(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${r}` : `${m}:${r}`;
}

export function ago(at: number, t = now()): string {
  const d = t - at;
  if (d < 5) return "just now";
  if (d < 60) return `${d}s ago`;
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  return `${Math.floor(d / 86400)}d ago`;
}

export function timeOfDay(at: number): string {
  return new Date(at * 1000).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/** Intercepta trait names read as words: wallet_drainer -> wallet drainer */
export function words(s: string | undefined): string {
  return (s ?? "").replace(/_/g, " ");
}
