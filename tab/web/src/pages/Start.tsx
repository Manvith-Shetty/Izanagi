import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, left, type Stage } from "../api";
import { Brand, Qr, useCountdown } from "../components/bits";

const STEPS = ["verify with World ID", "deploying your wallet", "binding it to your World ID", "adding your free trial funds"];

function Verifying({ stage }: { stage: Extract<Stage, { stage: "verifying" }> }) {
  const secs = useCountdown(stage.expires_at);
  return (
    <div style={{ display: "grid", gap: "1.2rem" }}>
      <p>Open this on your phone and approve with World ID. It checks you're a real, unique person. Tab never learns who you are.</p>
      <div style={{ display: "flex", gap: "1.4rem", alignItems: "center", flexWrap: "wrap" }}>
        <Qr value={stage.world_url} label="QR code for the World ID link" />
        <div style={{ display: "grid", gap: "0.6rem" }}>
          <a className="btn" href={stage.world_url} target="_blank" rel="noreferrer">
            Verify with World ID
          </a>
          <span className="muted" style={{ fontSize: "var(--t-sm)" }}>
            If asked for a code: <span className="code" style={{ fontSize: "var(--t-md)" }}>{stage.user_code}</span>
          </span>
          <span className="muted" style={{ fontSize: "var(--t-sm)" }}>Expires in {left(secs)}.</span>
        </div>
      </div>
    </div>
  );
}

export default function Start() {
  const nav = useNavigate();
  const [id, setId] = useState<string>();
  const [stage, setStage] = useState<Stage>();
  const [error, setError] = useState<string>();
  const started = useRef(false);

  const begin = () => {
    setError(undefined);
    setStage(undefined);
    api
      .signup()
      .then((r) => {
        setId(r.id);
        setStage(r.stage);
      })
      .catch((e) => setError(e.message));
  };

  useEffect(() => {
    // already signed in? straight to the dashboard
    api.me().then(() => nav("/app", { replace: true })).catch(() => {
      if (!started.current) {
        started.current = true;
        begin();
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!id || !stage || stage.stage === "ready" || stage.stage === "failed") return;
    const t = setTimeout(() => {
      api
        .signupPoll(id)
        .then((r) => {
          setStage(r.stage);
          if (r.stage.stage === "ready") setTimeout(() => nav("/app", { replace: true }), r.stage.returning ? 400 : 1200);
        })
        .catch((e) => setError(e.message));
    }, 2000);
    return () => clearTimeout(t);
  }, [id, stage, nav]);

  const current = stage?.stage === "creating" ? STEPS.indexOf(stage.step) : stage?.stage === "ready" ? STEPS.length : 0;

  return (
    <>
      <header className="wrap bar">
        <Brand />
      </header>
      <main className="wrap" style={{ display: "grid", gap: "2.5rem", gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))", padding: "2rem var(--gutter) 4rem", alignItems: "start" }}>
        <div style={{ display: "grid", gap: "1.2rem" }}>
          <h1 style={{ fontSize: "var(--t-2xl)" }}>Get your Tab</h1>
          {error && (
            <div className="error">
              {error}{" "}
              <button className="linkish" onClick={begin}>
                Start again
              </button>
            </div>
          )}
          {!stage && !error && <p className="muted">Asking World ID for a verification…</p>}
          {stage?.stage === "verifying" && <Verifying stage={stage} />}
          {stage?.stage === "creating" && <p>You're verified. Setting up your wallet on Base now; this takes a few seconds.</p>}
          {stage?.stage === "ready" && <p>{stage.returning ? "Welcome back. Opening your Tab." : "Your Tab is ready. Opening it."}</p>}
          {stage?.stage === "failed" && (
            <div className="error">
              {stage.reason}{" "}
              <button className="linkish" onClick={begin}>
                Try again
              </button>
            </div>
          )}
        </div>

        <div className="receipt" style={{ maxWidth: 420 }}>
          <div className="head">New Tab</div>
          <div className="sub">one per person</div>
          <hr />
          {STEPS.map((s, i) => (
            <div key={s} className={`line ${i < current ? "settled" : i === current ? "open" : ""}`}>
              <span className="what">{s}</span>
              <span className="amt">{i < current ? "done" : i === current && stage?.stage !== "failed" ? "…" : ""}</span>
            </div>
          ))}
          <hr />
          <div className="line total">
            <span className="what">Free to start</span>
            <span className="amt">$0.25</span>
          </div>
        </div>
      </main>
    </>
  );
}
