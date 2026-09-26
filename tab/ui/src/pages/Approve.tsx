import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Mark, SampleNotice } from "../components/Chrome";
import { loadApproval } from "../lib/api";
import { useNow } from "../lib/hooks";
import { clock, short, usdcShort } from "../lib/format";
import { explorer } from "../lib/wallet";

const denied: Record<string, string> = {
  access_denied: "You declined it in World App.",
  expired_token: "The code expired before it was used.",
  cancelled: "The request was cancelled.",
  stale_authentication: "The World ID proof was too old. A fresh one is needed for every payment.",
  insufficient_assurance: "This needs an Orb-verified World ID.",
};

export default function Approve() {
  const { id = "" } = useParams();
  const t = useNow();
  const [copied, setCopied] = useState(false);
  const q = useQuery({
    queryKey: ["approval", id],
    queryFn: () => loadApproval(id),
    refetchInterval: (query) => (query.state.data?.data.approval.status === "pending" && !query.state.data.sample ? 2000 : false),
  });

  const page = q.data?.data;
  const a = page?.approval;
  const who = page?.service ?? (a ? short(a.seller) : "");
  const sample = q.data?.sample ?? false;
  const left = a ? a.expiresAt - t : 0;
  const total = a ? Math.max(1, a.expiresAt - a.createdAt) : 1;
  const raw = a && a.status === "pending" && left <= 0 ? "expired" : a?.status;
  const status = raw === "used" ? "approved" : raw;
  const restore = a?.purpose === "restore";
  const worldLink = page?.worldUrl || a?.verificationUriComplete || a?.verificationUri;

  const copy = async () => {
    if (!a) return;
    try {
      await navigator.clipboard.writeText(a.userCode);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* still selectable */
    }
  };

  return (
    <div className="ground grain flex min-h-dvh flex-col">
      <SampleNotice show={sample} />
      <header className="mx-auto flex w-full max-w-[30rem] items-center gap-r3 px-r4 pt-r4">
        <Link to="/" className="-m-r2 flex items-center gap-r3 rounded-lg p-r2 transition-opacity duration-150 hover:opacity-85 active:opacity-70">
          <Mark className="size-7" />
          <span className="display text-[1.2rem] leading-none">Izanagi</span>
        </Link>
      </header>

      <main className="mx-auto w-full max-w-[30rem] flex-1 px-r4 pb-r6 pt-r5">
        {q.isError ? (
          <p className="text-beni-deep">{q.error instanceof Error ? q.error.message : "Tab couldn't load this approval."}</p>
        ) : q.isLoading || !a ? (
          <p className="text-stone">Loading the request…</p>
        ) : (
          <>
            <p className="text-[0.95rem] text-stone">{restore ? "Your agent wants to reopen a closed tab" : `Your agent wants to spend more with ${who}`}</p>
            {restore ? (
              <h1 className="display mt-r2 text-[2.3rem]">
                Start paying {page?.service ?? <span className="hex text-[0.7em] tracking-normal">{short(a.seller)}</span>} again?
              </h1>
            ) : (
              <h1 className="display num mt-r1 text-[3.6rem] leading-none">
                {usdcShort(a.amount)} <span className="text-[0.45em] tracking-normal text-stone">USDC</span>
              </h1>
            )}
            <dl className="mt-r4 divide-y divide-rule/70 border-y border-rule/70 text-[0.93rem]">
              {!restore && (
                <div className="flex gap-r3 py-r3">
                  <dt className="w-20 shrink-0 text-stone">To</dt>
                  <dd className="min-w-0 break-all text-sumi">
                    {page?.service && <span className="font-semibold">{page.service} </span>}
                    <span className="hex">{a.seller}</span>
                  </dd>
                </div>
              )}
              <div className="flex gap-r3 py-r3">
                <dt className="w-20 shrink-0 text-stone">Why</dt>
                <dd className="text-sumi">{a.reason}</dd>
              </div>
              <div className="flex gap-r3 py-r3">
                <dt className="w-20 shrink-0 text-stone">From</dt>
                <dd className="hex min-w-0 text-sumi">{short(a.wallet, 8, 6)}</dd>
              </div>
            </dl>

            <div aria-live="polite" className="mt-r5">
              {status === "pending" && (
                <section className="surface-float rounded-[1.25rem] p-r4">
                  <ol className="flex flex-col gap-r4">
                    <li className="grid grid-cols-[1.75rem_1fr] gap-r3">
                      <span className="num flex size-7 items-center justify-center rounded-full bg-sumi text-[0.85rem] font-semibold text-paper">1</span>
                      <div>
                        <p className="font-semibold">Open World App</p>
                        <a href={worldLink} target="_blank" rel="noreferrer" className="btn btn-primary mt-r2 w-full">
                          Approve with World ID
                        </a>
                      </div>
                    </li>
                    <li className="grid grid-cols-[1.75rem_1fr] gap-r3">
                      <span className="num flex size-7 items-center justify-center rounded-full bg-sumi text-[0.85rem] font-semibold text-paper">2</span>
                      <div>
                        <p className="font-semibold">Enter this code if asked</p>
                        <button
                          type="button"
                          onClick={copy}
                          className="group mt-r2 flex w-full items-center justify-between rounded-xl bg-mist-deep px-r4 py-r3 transition-transform duration-200 ease-[var(--ease-spring)] hover:-translate-y-px active:translate-y-px"
                        >
                          <span className="hex text-[1.5rem] font-medium tracking-[0.14em] text-sumi">{a.userCode}</span>
                          <span className="text-[0.85rem] text-stone group-hover:text-sumi">{copied ? "Copied" : "Copy"}</span>
                        </button>
                      </div>
                    </li>
                  </ol>
                  <div className="mt-r4">
                    <div className="h-1 overflow-hidden rounded-full bg-mist-deep">
                      <div
                        className="h-full origin-left rounded-full bg-kin"
                        style={{ transform: `scaleX(${Math.max(0, left) / total})`, transition: "transform 1s linear" }}
                      />
                    </div>
                    <p className="num mt-r2 text-[0.85rem] text-stone">Expires in {clock(left)}. Waiting for you.</p>
                  </div>
                </section>
              )}

              {status === "approved" && (
                <section className="rounded-[1.25rem] bg-tide p-r5 text-paper" style={{ boxShadow: "var(--shadow-float)" }}>
                  <p className="display text-[1.8rem] tracking-[-0.02em]">Approved</p>
                  <p className="mt-r2 text-paper/85">
                    {restore ? "The tab is open again. Your agent can pay this seller." : "Your agent may now spend up to this amount on this tab. It has been told."}
                    {a.orbVerified ? " Verified with your Orb World ID." : ""}
                  </p>
                  {a.tx && (
                    <a className="mt-r3 inline-block text-paper underline decoration-paper/40 underline-offset-4 transition-opacity hover:decoration-paper active:opacity-70" href={`${explorer}/tx/${a.tx}`} target="_blank" rel="noreferrer">
                      View the transaction
                    </a>
                  )}
                </section>
              )}

              {(status === "denied" || status === "expired") && (
                <section className="rounded-[1.25rem] bg-beni-wash p-r5 ring-1 ring-beni/25">
                  <p className="display text-[1.8rem] tracking-[-0.02em] text-beni-deep">{status === "expired" ? "Expired" : "Not approved"}</p>
                  <p className="mt-r2 text-sumi-soft">
                    {status === "expired" ? "Nobody approved this in time." : (denied[a.deniedReason ?? ""] ?? a.deniedReason ?? "The request ended without a yes.")}{" "}
                    {restore ? "The tab stays closed." : "No payment was made, and none can be."}
                  </p>
                </section>
              )}
            </div>

            <p className="mt-r5 text-[0.88rem] leading-relaxed text-stone">
              {restore
                ? "Approving reopens this one tab. Vouchers from before it was closed stay void."
                : "You're approving this tab only: up to this amount, with this seller. It can't be reused for anything else."}{" "}
              Izanagi checks your World ID on its server and never sees your identity, only that a verified person said yes.
            </p>
          </>
        )}
      </main>
    </div>
  );
}
