import { useEffect, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Mark } from "../components/Chrome";
import { Qr } from "../components/Qr";
import { loadMe, signup, signupPoll } from "../lib/api";
import { useNow } from "../lib/hooks";
import { clock } from "../lib/format";
import type { Stage } from "../lib/types";

// The server's own names for the steps after World ID (tab/src/onboard.rs), in order.
const STEPS = ["verify with World ID", "deploying your wallet", "binding it to your World ID", "adding your free trial funds"];
const LABELS = ["Prove you're a person with World ID", "Deploy your wallet on Base", "Bind the wallet to your World ID", "Add your free trial funds"];

export default function Start() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const t = useNow();
  const [id, setId] = useState<string>();
  const [stage, setStage] = useState<Stage>();
  const [error, setError] = useState<string>();
  const [copied, setCopied] = useState(false);
  const started = useRef(false);

  const begin = () => {
    setError(undefined);
    setStage(undefined);
    signup()
      .then((r) => {
        setId(r.id);
        setStage(r.stage);
      })
      .catch((e: Error) => setError(e.message));
  };

  // already signed in? straight to the dashboard
  useEffect(() => {
    loadMe().then((s) => {
      if (s.kind === "signed_in") navigate("/dashboard", { replace: true });
      else if (s.kind === "sample") setError("Tab isn't answering. Start it with cargo run -p tab, then try again.");
      else if (!started.current) {
        started.current = true;
        begin();
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!id || !stage || stage.stage === "ready" || stage.stage === "failed") return;
    const timer = setTimeout(() => {
      signupPoll(id)
        .then((r) => {
          setStage(r.stage);
          if (r.stage.stage === "ready") {
            qc.invalidateQueries({ queryKey: ["me"] });
            setTimeout(() => navigate("/dashboard", { replace: true }), r.stage.returning ? 400 : 1200);
          }
        })
        .catch((e: Error) => setError(e.message));
    }, 2000);
    return () => clearTimeout(timer);
  }, [id, stage, navigate, qc]);

  const current = stage?.stage === "creating" ? Math.max(1, STEPS.indexOf(stage.step)) : stage?.stage === "ready" ? STEPS.length : 0;
  const failed = stage?.stage === "failed" ? stage.reason : error;

  const copy = async (code: string) => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* still selectable */
    }
  };

  return (
    <div className="ground grain flex min-h-dvh flex-col">
      <header className="mx-auto flex w-full max-w-[30rem] items-center gap-r3 px-r4 pt-r4">
        <Link to="/" className="-m-r2 flex items-center gap-r3 rounded-lg p-r2 transition-opacity duration-150 hover:opacity-85 active:opacity-70">
          <Mark className="size-7" />
          <span className="display text-[1.2rem] leading-none">Izanagi</span>
        </Link>
      </header>

      <main className="mx-auto w-full max-w-[30rem] flex-1 px-r4 pb-r6 pt-r5">
        <h1 className="display text-[2.4rem]">Sign in with World ID</h1>
        <p className="mt-r2 text-[0.95rem] text-sumi-soft">
          One person gets one wallet. World ID checks you're a real, unique person; Izanagi never learns who you are.
        </p>

        <div aria-live="polite" className="mt-r5">
          {failed ? (
            <section className="rounded-[1.25rem] bg-beni-wash p-r5 ring-1 ring-beni/25">
              <p className="font-semibold text-beni-deep">Signing in didn't finish</p>
              <p className="mt-r2 text-sumi-soft">{failed}</p>
              <button type="button" className="btn btn-quiet mt-r4" onClick={begin}>
                Try again
              </button>
            </section>
          ) : stage?.stage === "verifying" ? (
            <section className="surface-float rounded-[1.25rem] p-r4">
              <div className="flex flex-wrap items-center gap-r4">
                <Qr value={stage.world_url} label="QR code that opens World App to verify" />
                <div className="min-w-0 flex-1">
                  <p className="font-semibold">Scan with your phone</p>
                  <p className="mt-r1 text-[0.9rem] text-sumi-soft">Or open the link on the phone that has World App.</p>
                  <a href={stage.world_url} target="_blank" rel="noreferrer" className="btn btn-primary mt-r3 w-full">
                    Open World App
                  </a>
                </div>
              </div>
              <button
                type="button"
                onClick={() => copy(stage.user_code)}
                className="group mt-r4 flex w-full items-center justify-between rounded-xl bg-mist-deep px-r4 py-r3 transition-transform duration-200 ease-[var(--ease-spring)] hover:-translate-y-px active:translate-y-px"
              >
                <span className="text-[0.85rem] text-stone">Code, if asked</span>
                <span className="hex text-[1.3rem] font-medium tracking-[0.14em] text-sumi">{stage.user_code}</span>
                <span className="text-[0.85rem] text-stone group-hover:text-sumi">{copied ? "Copied" : "Copy"}</span>
              </button>
              <p className="num mt-r3 text-[0.85rem] text-stone">Expires in {clock(stage.expires_at - t)}. Waiting for you.</p>
            </section>
          ) : !stage ? (
            <p className="text-stone">Asking World ID for a code…</p>
          ) : null}

          {(stage?.stage === "creating" || stage?.stage === "ready") && (
            <ol className="surface-raised flex flex-col gap-r3 rounded-[1.25rem] p-r4">
              {LABELS.map((label, i) => (
                <li key={label} className="grid grid-cols-[1.75rem_1fr] items-center gap-r3">
                  <span
                    className={`num flex size-7 items-center justify-center rounded-full text-[0.85rem] font-semibold ${
                      i < current ? "bg-tide text-paper" : i === current ? "bg-sumi text-paper" : "bg-mist-deep text-stone"
                    }`}
                  >
                    {i + 1}
                  </span>
                  <span className={i <= current ? "text-sumi" : "text-stone"}>{label}</span>
                </li>
              ))}
              <li className="mt-r1 text-[0.9rem] text-sumi-soft">
                {stage.stage === "ready" ? (stage.returning ? "Welcome back. Opening your tabs." : "Your wallet is ready. Opening your tabs.") : "This takes a few seconds."}
              </li>
            </ol>
          )}
        </div>
      </main>
    </div>
  );
}
