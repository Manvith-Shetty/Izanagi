import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api, usd, type Census } from "../api";
import { Brand } from "../components/bits";

const BASESCAN = "https://basescan.org/tx/";

// Real purchases from our Countersign wallet on Base mainnet, 26 Sep 2026.
const PROOF = [
  {
    what: "BTC 1-minute candles",
    who: "hyperextend",
    price: 2000,
    paid: `${BASESCAN}0xafe253d7ac946616eae09f8e025371174412a5c5a62bb4303c9021a1365f1a99`,
  },
  {
    what: "Ethereum block height",
    who: "onesource",
    price: 1000,
    paid: `${BASESCAN}0xd4d8fbb14bf4572dc5b35285646d64c6d3cf69a76d5b8dfbc3f2925c493eaec2`,
  },
];

// A payment stopped after it was made, on Base mainnet through Izanagi's own code, 27 Sep 2026.
const STOPPED = {
  bought: `${BASESCAN}0xc2b0aa6554de6e294c57b18c1fe1882de4a30a78faeecca40693bab5737ba86d`,
  revoked: `${BASESCAN}0x310343a3cc51ceb658faf8172739b59fcdbee605d011477b300d9aba4dba1994`,
};

type Decision = "paid" | "approved" | "refused" | "stopped";
const TAG: Record<Decision, string> = {
  paid: "checked, paid",
  approved: "over your limit, you approved",
  refused: "seller flagged, refused",
  stopped: "sold junk, stopped before collection",
};

// One of each decision Izanagi makes. The first two lines are the real mainnet purchases above.
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
      <div className="sub reveal">an example: every line is a decision Izanagi made</div>
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
        : `${guarded === 1 ? "One of those agents is" : `${guarded} of those agents are`} guarded by Izanagi. For the rest, every payment is final the moment it's signed.`}
    </p>
  );
}

const CHECKS: [string, string][] = [
  ["Who it's paying", "Before a single payment is signed, Intercepta screens the seller for scams, drainers and sanctions. A flagged seller is refused, and the reason is shown."],
  ["How much", "Each seller gets a limit. Under it, your AI just pays. Over it, the payment waits for you."],
  ["Who approves", "You do, with a fresh World ID check on your phone. Only your World ID can approve payments from your account."],
  ["Until it's collected", "Sellers collect later, and Izanagi keeps checking them. If one turns out to be a scam, its payments are stopped before the money leaves."],
];

/** The life of one payment, and where each kind of guard gets its last say. */
function Timeline() {
  const ref = useRef<HTMLDivElement>(null);
  const [seen, setSeen] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || !("IntersectionObserver" in window)) return setSeen(true);
    const io = new IntersectionObserver(
      ([e]) => {
        if (e.isIntersecting) {
          setSeen(true);
          io.disconnect();
        }
      },
      { threshold: 0.6 },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);

  return (
    <div className="timeline" ref={ref}>
      <div className="tl-stop signed">
        <span className="tl-dot" aria-hidden="true" />
        <h3>Your AI signs</h3>
        <p className="muted">
          Every other guard makes its last check here. Once the signature exists, the payment is final.
        </p>
      </div>
      <div className="tl-span">
        <span className="tl-track" aria-hidden="true" />
        <p>
          The seller serves your AI, and collects minutes or days later. All that time, Intercepta keeps re-screening it.
        </p>
      </div>
      <div className="tl-stop collected">
        <span className="tl-dot" aria-hidden="true" />
        <h3>The seller collects</h3>
        <p className="muted">
          Coinbase's escrow asks your Izanagi wallet whether the payment still stands. If the seller has turned bad, the
          answer is no, and it gets nothing.
        </p>
        {seen && <span className="stamp tl-stamp slam" aria-hidden="true">STOPPED</span>}
      </div>
    </div>
  );
}

// Each row is something a guard that only checks before signing cannot do, whoever builds it.
const LEDGER: [string, string, string][] = [
  [
    "A seller turns bad after your AI paid",
    "The payment is final",
    "Stopped: the seller's claim fails on chain, and the money stays yours",
  ],
  [
    "Fraud screening",
    "Once, before paying",
    "Before paying, then again and again until the seller collects",
  ],
  [
    "Who has the last word",
    "The agent, at the moment it signs",
    "Your wallet, at the moment the seller collects. Without the fraud check's signature, nothing can be collected",
  ],
];

function Ledger() {
  return (
    <div className="ledger-wrap">
      <table className="ledger">
        <thead>
          <tr>
            <th scope="col">
              <span className="sr-only">Question</span>
            </th>
            <th scope="col">A guard that checks before signing</th>
            <th scope="col">Izanagi</th>
          </tr>
        </thead>
        <tbody>
          {LEDGER.map(([q, them, us]) => (
            <tr key={q}>
              <th scope="row">{q}</th>
              <td className="them" data-label="A guard that checks before signing">{them}</td>
              <td className="us" data-label="Izanagi">{us}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function Landing() {
  return (
    <>
      <header className="wrap bar">
        <Brand />
        <nav>
          <a href="#checks" className="hide-sm">
            What it checks
          </a>
          <a href="#after" className="hide-sm">
            After your AI pays
          </a>
          <a href="#proof" className="hide-sm">
            On mainnet
          </a>
          <Link className="btn small" to="/start">
            Get started
          </Link>
        </nav>
      </header>

      <main>
        <section className="wrap" style={{ display: "grid", gap: "3rem", gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))", alignItems: "center", padding: "3rem var(--gutter) 4.5rem" }}>
          <div style={{ display: "grid", gap: "1.4rem" }}>
            <h1 className="hero-title">Every payment your AI makes, checked first.</h1>
            <p className="lede">
              Izanagi gives your AI its own spending account. It screens every seller, holds each payment to your limits, and
              asks you before anything bigger goes through.
            </p>
            <div style={{ display: "flex", gap: "0.8rem", flexWrap: "wrap", alignItems: "center" }}>
              <Link className="btn" to="/start">
                Get started
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
          <h2 style={{ marginBottom: "0.6rem" }}>What Izanagi checks, every payment</h2>
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

        <section id="after" style={{ background: "var(--paper)", padding: "4.5rem 0" }}>
          <div className="wrap" style={{ display: "grid", gap: "2.5rem" }}>
            <div style={{ display: "grid", gap: "0.8rem" }}>
              <h2 className="after-title">
                Other guards stop checking once your AI signs. <span className="nowrap">Izanagi doesn't.</span>
              </h2>
              <p className="muted">
                Agent payments on Base are signed first and collected later. Izanagi's wallet keeps a say over that whole gap,
                so a seller that turns out to be a scam after it was paid still can't collect.
              </p>
            </div>
            <Timeline />
            <Ledger />
          </div>
        </section>

        <section className="wrap" style={{ padding: "4rem var(--gutter)", display: "grid", gap: "2.5rem", gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))" }}>
          <div style={{ display: "grid", gap: "1rem", alignContent: "start" }}>
            <h2>How it works</h2>
            <p>
              <strong>Prove you're a person</strong> with World ID and create your own Izanagi wallet from MetaMask.{" "}
              <strong>Connect your AI</strong> by pasting your Izanagi link into Claude. Then <strong>let it pay</strong> for
              data and tools across the web.
            </p>
            <p>
              Small, clean payments just happen. Bigger ones ping your phone. Suspicious ones never get signed.
            </p>
          </div>
          <div style={{ display: "grid", gap: "1rem", alignContent: "start" }}>
            <h2>What Izanagi can't do</h2>
            <p>
              Izanagi can decline a payment, but it can never move your money. Its key only says no, and every payment also
              needs your AI's own signature. Your MetaMask owns the wallet from the block that created it.
            </p>
            <p>
              The final check runs on chain: when a seller collects, Coinbase's escrow asks your wallet whether the payment still
              stands. That answer comes from your wallet's rules, not from a server.
            </p>
          </div>
        </section>

        <section id="proof" style={{ background: "var(--paper)", padding: "4rem 0" }}>
          <div className="wrap" style={{ display: "grid", gap: "1.5rem" }}>
            <h2>Already running on Base mainnet</h2>
            <p className="muted">
              Real USDC, Coinbase's own escrow, and sellers nobody at Izanagi knows. Neither seller changed anything to accept
              an Izanagi wallet.
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
                      screened, paid, collected
                    </a>
                  </span>
                </li>
              ))}
              <li className="proof stopped">
                <span>BTC candle from hyperextend, then its tab closed</span>
                <span>
                  <a href={STOPPED.bought} target="_blank" rel="noreferrer">
                    paid
                  </a>{" "}
                  ·{" "}
                  <a href={STOPPED.revoked} target="_blank" rel="noreferrer">
                    stopped after signing
                  </a>
                </span>
              </li>
            </ul>
            <p className="muted" style={{ fontSize: "var(--t-sm)" }}>
              hyperextend served the data and still holds our signed payment for it. It hadn't collected when the tab was
              closed, so now it never can.
            </p>
            <div>
              <Link className="btn" to="/start">
                Get started
              </Link>
            </div>
          </div>
        </section>
      </main>

      <footer className="wrap footer">
        Izanagi, built at ETHGlobal Tokyo 2026 on Coinbase's x402 escrow, World ID and Intercepta.{" "}
        <a href="https://sourcify.dev/#/lookup/0x81E0FAC8aA64cE0E95Ec43337568c0744aB0C70b" target="_blank" rel="noreferrer">
          Read the wallet's code
        </a>
        {" or "}
        <a href="https://github.com/Manvith-Shetty/Izanagi" target="_blank" rel="noreferrer">
          the source
        </a>
        .
      </footer>
    </>
  );
}
