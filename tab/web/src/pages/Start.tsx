import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, left, type Stage } from "../api";
import { Brand, Qr, useCountdown } from "../components/bits";
import { connect, explain, send, sign } from "../metamask";

const STEPS = ["verify with World ID", "create your wallet with MetaMask", "tie it to your World ID"];

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

/** The person's own MetaMask signs for this signup, then creates their wallet and pays for it. */
function CreateWallet({ id, stage, onStage }: { id: string; stage: Extract<Stage, { stage: "create_wallet" }>; onStage: (s: Stage) => void }) {
  const [step, setStep] = useState<string>();
  const [error, setError] = useState<string>();

  const create = async () => {
    setError(undefined);
    try {
      setStep("Connecting MetaMask…");
      const from = await connect(stage.chain_id);
      setStep("Sign in MetaMask to say this signup is yours. It costs nothing.");
      const signature = await sign(from, stage.message);
      const { tx } = await api.signupWallet(id, from, signature);
      setStep("Confirm creating your wallet in MetaMask…");
      const hash = await send(from, tx);
      setStep("Waiting for your wallet to land on Base…");
      const r = await api.signupCreated(id, hash);
      onStage(r.stage);
    } catch (e) {
      setError(explain(e));
      setStep(undefined);
    }
  };

  return (
    <div style={{ display: "grid", gap: "1rem" }}>
      <p>
        You're verified. Now create your wallet from MetaMask: it belongs to your MetaMask account from the first block, and
        only that account can ever take money out. Tab never holds a key to it.
      </p>
      <div>
        <button className="btn" onClick={create} disabled={!!step}>
          Connect MetaMask & create your wallet
        </button>
      </div>
      <p className="muted" style={{ fontSize: "var(--t-sm)" }}>
        Two MetaMask prompts: a free signature, then the transaction that creates the wallet (a cent or less of ETH on Base).
      </p>
      {step && <div className="notice">{step}</div>}
      {error && <div className="error">{error}</div>}
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

  const advance = (s: Stage) => {
    setStage(s);
    if (s.stage === "ready") setTimeout(() => nav("/app", { replace: true }), s.returning ? 400 : 1200);
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

  // only World ID moves on its own; every later step is the person's
  useEffect(() => {
    if (!id || stage?.stage !== "verifying") return;
    const t = setTimeout(() => {
      api
        .signupPoll(id)
        .then((r) => advance(r.stage))
        .catch((e) => setError(e.message));
    }, 2000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, stage]);

  const current =
    stage?.stage === "create_wallet"
      ? 1
      : stage?.stage === "creating"
        ? stage.step.startsWith("tying") ? 2 : 1
        : stage?.stage === "ready"
          ? STEPS.length
          : 0;

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
          {stage?.stage === "create_wallet" && id && <CreateWallet id={id} stage={stage} onStage={advance} />}
          {stage?.stage === "creating" && <p>Your wallet is on Base. {stage.step[0].toUpperCase() + stage.step.slice(1)}…</p>}
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
            <span className="what">Owner</span>
            <span className="amt">you</span>
          </div>
        </div>
      </main>
    </>
  );
}
