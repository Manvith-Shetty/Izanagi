import { Link } from "react-router-dom";
import type { ReactNode } from "react";
import type { FeedItem } from "../lib/types";
import { short, timeOfDay, usdcShort, words } from "../lib/format";
import { explorer } from "../lib/wallet";

type Tone = "tide" | "kin" | "beni" | "sumi" | "stone";

const dot: Record<Tone, string> = {
  tide: "bg-tide",
  kin: "bg-kin",
  beni: "bg-beni",
  sumi: "bg-sumi",
  stone: "bg-stone-light",
};

function str(v: unknown): string {
  return typeof v === "string" ? v : v == null ? "" : String(v);
}

export function describe(i: FeedItem, name: (addr: string) => ReactNode): { tone: Tone; text: ReactNode; detail?: ReactNode } {
  const seller = name(str(i.seller));
  const amt = (k: string) => <span className="num font-semibold">{usdcShort(i[k] as number)} USDC</span>;
  switch (i.kind) {
    case "screened": {
      const score = <span className="num">score {Number(i.toxic_score ?? 0).toFixed(0)}</span>;
      switch (i.verdict) {
        case "pay":
          return { tone: "tide", text: <>Cleared {seller} for {amt("requested")}</>, detail: <>Intercepta: {words(str(i.reason))}, {score}</> };
        case "cap":
          return { tone: "kin", text: <>Capped a payment to {seller}</>, detail: <>{str(i.reason)}, {score}</> };
        case "ask":
          return { tone: "kin", text: <>Asked you about {amt("requested")} to {seller}</>, detail: str(i.reason) };
        default:
          return { tone: "beni", text: <>Refused to pay {seller}</>, detail: <>Intercepta: {words(str(i.reason))}, {score}</> };
      }
    }
    case "countersigned":
      return {
        tone: "tide",
        text: <>Countersigned up to {amt("ceiling")} for {seller}</>,
        detail: i.expiry ? <>Claimable until {timeOfDay(Number(i.expiry))}</> : undefined,
      };
    case "approval_requested":
      return {
        tone: "kin",
        text: (
          <>
            Waiting for you to approve {amt("amount")} to {seller}
          </>
        ),
        detail: (
          <Link className="link" to={`/approve/${str(i.approval_id)}`}>
            Open the approval
          </Link>
        ),
      };
    case "approval_granted":
      return { tone: "tide", text: <>Approved with World ID{i.orb_verified ? ", Orb-verified" : ""}</>, detail: str(i.purpose) === "restore" ? "Reopening a tab" : "One payment" };
    case "approval_denied":
      return { tone: "beni", text: <>Not approved: {words(str(i.reason))}</>, detail: "Nothing was signed" };
    case "human_bound":
      return { tone: "sumi", text: <>This wallet now answers to one verified person</> };
    case "revoked": {
      const by = str(i.by) === "watcher" ? "the watcher, after a score change" : str(i.by) === "human" ? "you, with World ID" : "you";
      const before = i.score_before != null && i.score_now != null ? <>, score {Number(i.score_before).toFixed(0)} to {Number(i.score_now).toFixed(0)}</> : null;
      return {
        tone: "beni",
        text: (
          <>
            Closed the tab with {seller}. {amt("value_stopped")} in {String(i.vouchers_stopped ?? 0)} signed vouchers will never be paid
          </>
        ),
        detail: (
          <>
            {words(str(i.reason))}, by {by}
            {before}
            {i.tx ? (
              <>
                {" "}
                <a className="link" href={`${explorer}/tx/${str(i.tx)}`} target="_blank" rel="noreferrer">
                  transaction
                </a>
              </>
            ) : null}
          </>
        ),
      };
    }
    case "restored":
      return { tone: "tide", text: <>Reopened the tab with {seller}</>, detail: "Approved by a person with World ID" };
    case "tab_opened":
      return { tone: "sumi", text: <>Opened a tab with {seller}</>, detail: <>Deposit {amt("deposit")}</> };
    case "purchase":
      return { tone: "stone", text: <>Paid {seller} {amt("price")}</>, detail: i.path ? <span className="hex">{str(i.path)}</span> : undefined };
    case "claimed":
      return { tone: "sumi", text: <>{seller} cashed in {amt("amount")}</> };
    case "claim_rejected":
      return { tone: "beni", text: <>The escrow rejected {seller}'s claim for {amt("amount")}</>, detail: str(i.reason) };
    default:
      return { tone: "stone", text: words(i.kind) };
  }
}

export function FeedLine({ item, name, fresh }: { item: FeedItem; name: (a: string) => ReactNode; fresh?: boolean }) {
  const d = describe(item, name);
  return (
    <li className={`grid grid-cols-[0.5rem_1fr_auto] gap-x-r3 py-r3 ${fresh ? "animate-[arrive_600ms_var(--ease-out-soft)]" : ""}`}>
      <span className={`mt-[0.55rem] size-2 rounded-full ${dot[d.tone]}`} aria-hidden="true" />
      <div className="min-w-0">
        <p className={`text-[0.93rem] leading-snug ${d.tone === "beni" ? "text-beni-deep" : "text-sumi"}`}>{d.text}</p>
        {d.detail && <p className="mt-[0.15rem] text-[0.84rem] leading-snug text-stone">{d.detail}</p>}
      </div>
      <time className="num pt-[0.1rem] text-[0.78rem] text-stone-light" dateTime={new Date(item.at * 1000).toISOString()}>
        {timeOfDay(item.at)}
      </time>
    </li>
  );
}

export const shortAddr = (a: string) => <span className="hex">{short(a)}</span>;
