import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, usd, type Census } from "../api";
import { Brand } from "../components/bits";

// Real purchases from our Countersign wallet on Base mainnet, 26 Sep 2026.
const PROOF = [
  {
    what: "BTC 1-minute candles",
    who: "hyperextend",
    price: 2000,
    paid: "https://basescan.org/tx/0xafe253d7ac946616eae09f8e025371174412a5c5a62bb4303c9021a1365f1a99",
  },
  {
    what: "Ethereum block height",
    who: "onesource",
    price: 1000,
    paid: "https://basescan.org/tx/0xd4d8fbb14bf4572dc5b35285646d64c6d3cf69a76d5b8dfbc3f2925c493eaec2",
  },
];

type Decision = "paid" | "approved" | "refused" | "stopped";
const TAG: Record<Decision, string> = {
  paid: "checked, paid",
  approved: "over your limit, you approved",
  refused: "seller flagged, refused",
  stopped: "sold junk, stopped before collection",
};

// One of each decision Tab makes. The first two lines are the real mainnet purchases above.
const EXAMPLE: { what: string; who: string; amt: number; d: Decision }[] = [
  { what: "BTC 1-minute candles", who: "hyperextend", amt: 2000, d: "paid" },
  { what: "Ethereum block height", who: "onesource", amt: 1000, d: "paid" },
  { what: "Market data, 20 calls", who: "a data API", amt: 50000, d: "approved" },
  { what: "\"Premium API access\"", who: "0x9f3a…e21c", amt: 5_000_000, d: "refused" },
  { what: "BTC price, 3 calls", who: "a shop gone bad", amt: 3000, d: "stopped" },
];

function HeroReceipt() {
  return (
    <div className="receipt printing" style={{ maxWidth: 440, width: "100%" }} aria-label="Example receipt">
      <div className="head reveal">Your AI's payments</div>
      <div className="sub reveal">an example: every line is a decision Tab made</div>
      <hr />
      {EXAMPLE.map((l, i) => (
        <div key={i} className={`line ${l.d === "refused" || l.d === "stopped" ? "void" : ""}`} style={{ animationDelay: `${250 + i * 240}ms` }}>
          <span className="what">
            {l.what}
            <span className="note">
              {l.who} · <span className={`decision ${l.d}`}>{TAG[l.d]}</span>
            </span>
          </span>
          <span className="amt">{usd(l.amt)}</span>
        </div>
      ))}
      <hr />
      <div className="line total">
        <span className="what">Paid</span>
        <span className="amt">{usd(53000)}</span>
      </div>
      <div className="line">
        <span className="what">Refused or stopped</span>
        <span className="amt" style={{ color: "var(--stamp)" }}>{usd(5_003_000)}</span>
      </div>
    </div>
  );
}

function CensusLine() {
  const [c, setC] = useState<Census>();
  useEffect(() => {
    api.network().then((r) => r.census.channels > 0 && setC(r.census)).catch(() => {});
  }, []);
  if (!c) {
    return <p className="lede">AI agents already pay each other on Base, thousands of times a week, and almost every one of those payments is final the moment it's signed.</p>;
  }
  const guarded = c.countersignPayers;
  return (
    <p className="lede" style={{ maxWidth: "46rem" }}>
      Right now on Base, <strong>{c.payers.toLocaleString()}</strong> AI agents have opened{" "}
      <strong>{c.channels.toLocaleString()}</strong> payment tabs with <strong>{c.sellers}</strong> sellers, holding{" "}
      <strong>{usd(c.escrowUsdc, true)}</strong>.{" "}
      {guarded === 0
        ? "Every one of those payments was final the moment it was signed."
        : `${guarded === 1 ? "One of those agents is" : `${guarded} of those agents are`} guarded by Tab. For the rest, every payment is final the moment it's signed.`}
    </p>
  );
}

const CHECKS: [string, string][] = [
  ["Who it's paying", "Before a single payment is signed, Intercepta screens the seller for scams, drainers and sanctions. A flagged seller is refused, and the reason is shown."],
  ["How much", "Each seller gets a limit. Under it, your AI just pays. Over it, the payment waits for you."],
  ["Who approves", "You do, with a fresh World ID check on your phone. Only your World ID can approve payments from your account."],
  ["Until it's collected", "Sellers collect later, and Tab keeps checking them. If one turns out to be a scam, its payments are stopped before the money leaves."],
];

export default function Landing() {
  return (
    <>
      <header className="wrap bar">
        <Brand />
        <nav>
          <a href="#checks" className="hide-sm">
            What it checks
          </a>
          <a href="#proof" className="hide-sm">
            On mainnet
          </a>
          <Link className="btn small" to="/start">
            Get your Tab
          </Link>
        </nav>
      </header>

      <main>
        <section className="wrap" style={{ display: "grid", gap: "3rem", gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))", alignItems: "center", padding: "3rem var(--gutter) 4.5rem" }}>
          <div style={{ display: "grid", gap: "1.4rem" }}>
            <h1 className="hero-title">Every payment your AI makes, checked first.</h1>
            <p className="lede">
              Tab gives your AI its own spending account. It screens every seller, holds each payment to your limits, and asks
              you before anything bigger goes through.
            </p>
            <div style={{ display: "flex", gap: "0.8rem", flexWrap: "wrap", alignItems: "center" }}>
              <Link className="btn" to="/start">
                Get your Tab
              </Link>
              <span className="muted" style={{ fontSize: "var(--t-sm)" }}>
                One per person, verified with World ID.
              </span>
            </div>
          </div>
          <div style={{ display: "flex", justifyContent: "center" }}>
            <HeroReceipt />
          </div>
        </section>

        <section style={{ background: "var(--counter-deep)", padding: "2.4rem 0" }}>
          <div className="wrap">
            <CensusLine />
          </div>
        </section>

        <section id="checks" className="wrap" style={{ padding: "4rem var(--gutter)" }}>
          <h2 style={{ marginBottom: "0.6rem" }}>What Tab checks, every payment</h2>
          <p className="muted" style={{ marginBottom: "2rem" }}>
            Four questions, answered before and after your AI pays.
          </p>
          <ol className="ticket four">
            {CHECKS.map(([title, body]) => (
              <li key={title}>
                <h3>{title}</h3>
                <p className="muted">{body}</p>
              </li>
            ))}
          </ol>
        </section>

        <section className="wrap" style={{ padding: "0 var(--gutter) 4rem", display: "grid", gap: "2.5rem", gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))" }}>
          <div style={{ display: "grid", gap: "1rem", alignContent: "start" }}>
            <h2>How it works</h2>
            <p>
              <strong>Prove you're a person</strong> with World ID and get your own Tab. <strong>Connect your AI</strong> by
              pasting your Tab link into Claude. Then <strong>let it pay</strong> for data and tools across the web.
            </p>
            <p>
              Small, clean payments just happen. Bigger ones ping your phone. Suspicious ones never get signed.
            </p>
          </div>
          <div style={{ display: "grid", gap: "1rem", alignContent: "start" }}>
            <h2>What Tab can't do</h2>
            <p>
              Tab can decline a payment, but it can never move your money. Its key only says no, and every payment also needs your
              AI's own signature.
            </p>
            <p>
              The final check runs on chain: when a seller collects, Coinbase's escrow asks your wallet whether the payment still
              stands. That answer comes from your wallet's rules, not from a server.
            </p>
          </div>
        </section>

        <section id="proof" style={{ background: "var(--paper)", padding: "4rem 0" }}>
          <div className="wrap" style={{ display: "grid", gap: "1.5rem" }}>
            <h2>Already running on Base</h2>
            <p className="muted">
              Our wallet paid two sellers nobody at Tab knows, on mainnet, with real USDC. Both were screened first, and when
              each one collected, Coinbase's escrow checked with our wallet before paying out.
            </p>
            <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "grid", gap: "0.8rem" }}>
              {PROOF.map((p) => (
                <li key={p.paid} className="proof">
                  <span>
                    {p.what} from {p.who}
                  </span>
                  <span>
                    {usd(p.price)} ·{" "}
                    <a href={p.paid} target="_blank" rel="noreferrer">
                      collected, on Basescan
                    </a>
                  </span>
                </li>
              ))}
            </ul>
            <div>
              <Link className="btn" to="/start">
                Get your Tab
              </Link>
            </div>
          </div>
        </section>
      </main>

      <footer className="wrap footer">
        Built at ETHGlobal Tokyo 2026 on Coinbase's x402 escrow, World ID and Intercepta.{" "}
        <a href="https://sourcify.dev/#/lookup/0x81E0FAC8aA64cE0E95Ec43337568c0744aB0C70b" target="_blank" rel="noreferrer">
          Read the wallet's code
        </a>
        .
      </footer>
    </>
  );
}
