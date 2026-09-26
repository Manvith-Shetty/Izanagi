import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { api, left, scan, short, usd, type ApprovalView } from "../api";
import { Brand, Qr, useCountdown } from "../components/bits";

/** What the person is being asked, in their words. */
function ask(a: ApprovalView, service: string | null): { title: string; body: string } {
  const who = service ?? short(a.seller);
  switch (a.purpose) {
    case "payment":
      return {
        title: `Let your AI spend up to ${usd(a.amount)} with ${who}?`,
        body: "It asked for more than it may spend on its own. Nothing is paid unless you approve, and you can still close the tab afterwards.",
      };
    case "restore":
      return {
        title: `Reopen your tab with ${who}?`,
        body: "You closed this tab. If you approve, your AI can pay this seller again. If you don't, nothing changes.",
      };
    default:
      return { title: "Verify you're a person", body: "This sets up your own Izanagi account. It checks you're a real, unique person; Izanagi never learns who you are." };
  }
}

export default function Approve() {
  const { id = "" } = useParams();
  const [a, setA] = useState<ApprovalView>();
  const [service, setService] = useState<string | null>(null);
  const [world, setWorld] = useState<string>();
  const [error, setError] = useState<string>();
  const secs = useCountdown(a?.expiresAt ?? 0);

  useEffect(() => {
    let stop = false;
    const tick = () =>
      api
        .approval(id)
        .then((r) => {
          if (stop) return;
          setA(r.approval);
          setService(r.service);
          setWorld(r.worldUrl);
          if (r.approval.status === "pending") setTimeout(tick, 2000);
        })
        .catch((e) => !stop && setError(e.status === 404 ? "This approval doesn't exist, or has been cleared." : e.message));
    tick();
    return () => {
      stop = true;
    };
  }, [id]);

  const done = a && a.status !== "pending";
  const yes = a && (a.status === "approved" || a.status === "used");
  const q = a && ask(a, service);

  return (
    <>
      <header className="wrap bar">
        <Brand />
        <Link to="/app" style={{ fontSize: "var(--t-sm)" }}>
          Your account
        </Link>
      </header>
      <main className="wrap" style={{ display: "flex", justifyContent: "center", padding: "1.5rem var(--gutter) 4rem" }}>
        <div className="receipt" style={{ width: "100%", maxWidth: 520 }}>
          {error && <div className="error" style={{ fontFamily: "var(--display)" }}>{error}</div>}
          {!a && !error && <div className="muted">Loading…</div>}
          {a && q && (
            <div style={{ position: "relative" }}>
              <div className="head" style={{ fontSize: "var(--t-xl)", lineHeight: 1.1 }}>
                {q.title}
              </div>
              <p className="muted" style={{ fontFamily: "var(--display)", margin: "0.6rem 0 0" }}>
                {q.body}
              </p>
              <hr />
              {a.purpose !== "enroll" && (
                <>
                  <div className="line">
                    <span className="what">Seller</span>
                    <span className="amt">{service ?? short(a.seller)}</span>
                  </div>
                  {a.purpose === "payment" && (
                    <div className="line">
                      <span className="what">New limit for this tab</span>
                      <span className="amt">{usd(a.amount)}</span>
                    </div>
                  )}
                  <div className="line">
                    <span className="what">
                      Why you're asked
                      <span className="note">{a.reason}</span>
                    </span>
                    <span className="amt" />
                  </div>
                  <div className="line">
                    <span className="what">Wallet</span>
                    <span className="amt">{short(a.wallet)}</span>
                  </div>
                  <hr />
                </>
              )}

              {!done && world && (
                <div style={{ display: "flex", gap: "1.2rem", alignItems: "center", flexWrap: "wrap", fontFamily: "var(--display)" }}>
                  <Qr value={world} label="QR code to approve with World ID" />
                  <div style={{ display: "grid", gap: "0.55rem" }}>
                    <a className="btn" href={world} target="_blank" rel="noreferrer">
                      Approve with World ID
                    </a>
                    <span className="muted" style={{ fontSize: "var(--t-sm)" }}>
                      Code <span className="code" style={{ fontSize: "var(--t-md)" }}>{a.userCode}</span> · expires in {left(secs)}
                    </span>
                    <span className="muted" style={{ fontSize: "var(--t-sm)" }}>To say no, deny it there or just let it expire.</span>
                  </div>
                </div>
              )}

              {done && (
                <div style={{ minHeight: 90, position: "relative", fontFamily: "var(--display)" }}>
                  <span className={`stamp slam ${yes ? "ok" : ""}`} style={{ left: 8, top: 10 }}>
                    {yes ? "APPROVED" : a.status === "expired" ? "EXPIRED" : "DENIED"}
                  </span>
                  <p style={{ paddingTop: 70 }}>
                    {yes
                      ? a.purpose === "restore"
                        ? "Reopened. Your AI can pay this seller again."
                        : "Approved. Your AI can carry on."
                      : `Nothing happened${a.deniedReason ? ` (${a.deniedReason.replaceAll("_", " ")})` : ""}. No money moved.`}
                    {a.tx && scan("tx", a.tx, false) && (
                      <>
                        {" "}
                        <a href={scan("tx", a.tx, false)!} target="_blank" rel="noreferrer">
                          See it on Basescan
                        </a>
                        .
                      </>
                    )}
                  </p>
                </div>
              )}
            </div>
          )}
        </div>
      </main>
    </>
  );
}
