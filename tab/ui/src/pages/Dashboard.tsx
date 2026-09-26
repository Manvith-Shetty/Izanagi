import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, clock, left, scan, short, usd, type Listing, type Me, type Outcome, type Tab } from "../api";
import { Brand, CopyField, useCountdown } from "../components/bits";
import { Money } from "../components/Money";

type FeedItem = { id: number; at: number; kind: string; [k: string]: unknown };

/** One event from the live feed, in plain words. `null` for events not worth a line. */
function describe(i: FeedItem, name: (seller: string) => string): { text: string; stop?: boolean } | null {
  const seller = typeof i.seller === "string" ? name(i.seller) : "a seller";
  switch (i.kind) {
    case "screened":
      return i.verdict === "pay" ? null : { text: `Checked ${seller}: ${i.verdict}. ${i.reason}` };
    case "purchase":
      return { text: `Paid ${usd(i.price as number)} to ${seller}.` };
    case "purchase_refused":
      return { text: `Refused to pay ${seller}: ${i.reason}`, stop: true };
    case "tab_opened":
      return { text: `Opened a ${usd(i.deposit as number)} tab with ${seller}.` };
    case "tab_topped_up":
      return { text: `Added ${usd(i.deposit as number)} to your tab with ${seller}.` };
    case "approval_needed":
    case "approval_requested":
      return i.purpose === "enroll" ? null : { text: `Waiting for your approval (${i.purpose === "restore" ? "reopen a tab" : "raise a tab's limit"}).` };
    case "approval_granted":
      return { text: "You approved it with World ID." };
    case "approval_denied":
      return { text: `Not approved: ${String(i.reason).replaceAll("_", " ")}.`, stop: true };
    case "human_bound":
      return { text: "Your wallet is now tied to your World ID." };
    case "revoked":
      return { text: `Closed your tab with ${seller}. Its uncollected ${usd((i.value_stopped as number) ?? 0)} won't be paid.`, stop: true };
    case "restored":
      return { text: `Reopened your tab with ${seller}.` };
    case "seller_claimed":
      return { text: `${seller} cashed in ${usd(i.amount as number)}. That part is final now.` };
    case "account_created":
      return { text: `You created your wallet. It belongs to ${short(String(i.owner))}.` };
    case "deposited":
      return { text: `You added ${usd(i.amount as number)} from ${short(String(i.from))}.` };
    default:
      return null;
  }
}

function TabReceipt({ tab, me, onChange }: { tab: Tab; me: Me; onChange: () => void }) {
  const secs = useCountdown(tab.expiry);
  const [busy, setBusy] = useState(false);
  const [justStamped, setJustStamped] = useState(false);
  const [error, setError] = useState<string>();
  const nav = useNavigate();
  const lines = me.receipts.filter((r) => r.seller.toLowerCase() === tab.seller.toLowerCase()).slice(0, 8);
  const name = tab.service ?? short(tab.seller);
  const openLines = Math.max(0, Math.round((tab.charged - tab.claimed) / Math.max(tab.price, 1)));

  const close = async () => {
    setBusy(true);
    setError(undefined);
    try {
      await api.close(tab.seller, "closed from the dashboard");
      setJustStamped(true);
      onChange();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const reopen = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const a = await api.reopen(tab.seller);
      nav(`/approve/${a.id}`);
    } catch (e) {
      setError((e as Error).message);
      setBusy(false);
    }
  };

  return (
    <article className="receipt" style={{ marginBottom: 22 }}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: "1rem", alignItems: "start" }}>
        <div>
          <div className="head">{name}</div>
          <div className="sub">
            {short(tab.seller)} · {usd(tab.price)} a call
            {tab.riskScore != null ? ` · risk ${tab.riskScore}` : ""}
          </div>
        </div>
        <span className={`chip ${tab.status === "closed" ? "closed" : tab.status === "open" ? "open" : ""}`}>
          {tab.status === "closed" ? "Closed" : tab.status === "open" ? "Open" : "Lapsed"}
        </span>
      </div>
      <hr />
      <div style={{ position: "relative" }}>
        {lines.length === 0 && <div className="muted">No purchases yet.</div>}
        {lines.map((r, i) => {
          // the newest `openLines` purchases are the ones the seller has not cashed in yet
          const uncashed = i < openLines;
          const open = uncashed && tab.status === "open";
          const stopped = uncashed && tab.status === "closed";
          return (
            <div key={`${r.at}-${i}`} className={`line ${open ? "open" : "settled"} ${stopped ? "void" : ""}`}>
              <span className="what">
                {clock(r.at)} · call {Math.round(r.cumulative / Math.max(r.price, 1))}
                <span className="note">
                  {stopped ? "screened, signed · stopped before collection" : r.verdict === "pay" ? "screened, paid" : r.verdict}
                </span>
              </span>
              <span className="amt">{usd(r.price)}</span>
            </div>
          );
        })}
        {tab.status === "closed" && tab.saved > 0 && (
          <span className={`stamp ${justStamped ? "slam" : ""}`} style={{ right: 4, top: 4 }}>
            STOPPED
          </span>
        )}
      </div>
      <hr />
      <div className="line">
        <span className="what">Charged</span>
        <span className="amt">{usd(tab.charged)}</span>
      </div>
      <div className="line settled">
        <span className="what">Cashed in by the seller</span>
        <span className="amt">{usd(tab.claimed)}</span>
      </div>
      {tab.status === "open" && (
        <div className="line open">
          <span className="what">
            Not yet collected
            <span className="note">{tab.stoppable > 0 ? `can still be stopped · the seller may collect for ${left(secs)} more` : "nothing outstanding"}</span>
          </span>
          <span className="amt">{usd(tab.stoppable)}</span>
        </div>
      )}
      {tab.status === "closed" && (
        <div className="line total" style={{ color: "var(--stamp)" }}>
          <span className="what">Stopped before collection</span>
          <span className="amt">{usd(tab.saved)}</span>
        </div>
      )}
      <div style={{ display: "flex", gap: "0.6rem", marginTop: "1rem", flexWrap: "wrap", fontFamily: "var(--display)" }}>
        {tab.status === "open" ? (
          <button className="btn small stop" disabled={busy} onClick={close}>
            {busy ? "Closing…" : "Close tab"}
          </button>
        ) : (
          <button className="btn small ghost" disabled={busy} onClick={reopen}>
            {busy ? "Asking World ID…" : "Reopen with World ID"}
          </button>
        )}
        {tab.openTx && scan("tx", tab.openTx, me.network.fork) && (
          <a className="btn small ghost" href={scan("tx", tab.openTx, me.network.fork)!} target="_blank" rel="noreferrer">
            See it on Basescan
          </a>
        )}
      </div>
      {error && (
        <div className="error" style={{ marginTop: "0.8rem", fontFamily: "var(--display)" }}>
          {error}
        </div>
      )}
    </article>
  );
}

function TryIt({ listings, onDone }: { listings: Listing[]; onDone: () => void }) {
  const [busy, setBusy] = useState<string>();
  const [result, setResult] = useState<Outcome>();
  const waiting = useRef<string | undefined>(undefined);

  const buy = async (url: string) => {
    setBusy(url);
    setResult(undefined);
    try {
      let o = await api.buy(url);
      setResult(o);
      // a person must approve: keep waiting, a request at a time
      while (o.outcome === "needs_approval" || o.outcome === "still_pending") {
        const id = o.outcome === "needs_approval" ? o.approvalId : o.approval_id;
        waiting.current = id;
        o = await api.wait(id);
        if (waiting.current !== id) return;
        if (o.outcome !== "still_pending") setResult(o);
      }
    } catch (e) {
      setResult({ outcome: "unpayable", reason: (e as Error).message });
    } finally {
      setBusy(undefined);
      onDone();
    }
  };

  return (
    <div className="panel">
      <h3>Try it</h3>
      <p className="muted" style={{ fontSize: "var(--t-sm)", marginBottom: "0.8rem" }}>
        Buy one call yourself, exactly the way your AI would.
      </p>
      <div style={{ display: "grid", gap: "0.5rem" }}>
        {listings.filter((l) => l.source === "curated").map((l) => (
          <button key={l.url} className="btn small ghost" style={{ justifyContent: "space-between" }} disabled={!!busy} onClick={() => buy(l.url)}>
            <span>{l.name}</span>
            <span className="mono">{busy === l.url ? "…" : usd(l.price)}</span>
          </button>
        ))}
      </div>
      {result && (
        <div style={{ marginTop: "0.9rem", fontSize: "var(--t-sm)" }}>
          {result.outcome === "paid" && (
            <div className="notice">
              Paid {usd(result.price)} to {result.service ?? short(result.seller)}. It got back:
              <pre className="mono" style={{ whiteSpace: "pre-wrap", margin: "0.5rem 0 0", maxHeight: 140, overflow: "auto" }}>
                {JSON.stringify(result.data, null, 1).slice(0, 600)}
              </pre>
            </div>
          )}
          {result.outcome === "needs_approval" && (
            <div className="notice">
              This one needs your OK: {result.reason}.{" "}
              <a href={`/approve/${result.approvalId}`} target="_blank" rel="noreferrer">
                Approve it with World ID
              </a>
              . Waiting…
            </div>
          )}
          {(result.outcome === "refused" || result.outcome === "unpayable") && <div className="error">Not paid: {result.reason}</div>}
          {result.outcome === "denied" && <div className="error">Not approved ({result.reason.replaceAll("_", " ")}). Nothing was paid.</div>}
        </div>
      )}
    </div>
  );
}

export default function Dashboard() {
  const nav = useNavigate();
  const [me, setMe] = useState<Me>();
  const [listings, setListings] = useState<Listing[]>([]);
  const [feed, setFeed] = useState<FeedItem[]>([]);
  const [error, setError] = useState<string>();
  const refreshing = useRef<number | undefined>(undefined);

  const refresh = useCallback(() => {
    api
      .me()
      .then(setMe)
      .catch((e) => (e.status === 401 ? nav("/start", { replace: true }) : setError(e.message)));
  }, [nav]);

  useEffect(() => {
    refresh();
    api.catalog().then((r) => setListings(r.listings)).catch(() => {});
    const es = new EventSource("/api/stream");
    es.addEventListener("item", (e) => {
      const item = JSON.parse((e as MessageEvent).data) as FeedItem;
      setFeed((f) => (f.some((x) => x.id === item.id) ? f : [item, ...f].slice(0, 60)));
      window.clearTimeout(refreshing.current);
      refreshing.current = window.setTimeout(refresh, 500);
    });
    return () => es.close();
  }, [refresh]);

  if (error) return <main className="wrap" style={{ padding: "3rem var(--gutter)" }}><div className="error">{error}</div></main>;
  if (!me) return <main className="wrap" style={{ padding: "3rem var(--gutter)" }}><p className="muted">Opening your account…</p></main>;

  const name = (seller: string) =>
    me.tabs.find((t) => t.seller.toLowerCase() === seller.toLowerCase())?.service ?? short(seller);
  const walletLink = scan("address", me.account.wallet, me.network.fork);

  return (
    <>
      <header className="wrap bar">
        <Brand />
        <nav>
          <span className="chip live hide-sm">{me.network.fork ? "Base fork" : "Base mainnet"}</span>
          <span className="mono hide-sm">{short(me.account.wallet)}</span>
          <button className="linkish" onClick={() => api.logout().then(() => nav("/"))}>
            Sign out
          </button>
        </nav>
      </header>

      <main className="wrap" style={{ display: "grid", gap: 28, gridTemplateColumns: "repeat(auto-fit, minmax(320px, 1fr))", paddingBottom: "4rem", alignItems: "start" }}>
        <section style={{ gridColumn: "span 1" }}>
          <h2 style={{ margin: "0.5rem 0 1.2rem" }}>Your tabs</h2>
          {me.approvals.filter((a) => a.status === "pending").map((a) => (
            <div key={a.id} className="notice" style={{ marginBottom: 16 }}>
              Your OK is needed:{" "}
              {a.purpose === "restore" ? "reopen a tab" : `raise a tab to ${usd(a.amount)}`}.{" "}
              <Link to={`/approve/${a.id}`}>Review and approve</Link>
            </div>
          ))}
          {me.tabs.length === 0 ? (
            <div className="receipt">
              <div className="head">No tabs yet</div>
              <p className="muted" style={{ marginTop: "0.5rem", fontFamily: "var(--display)" }}>
                A tab opens the first time you or your AI buy from a seller. Try one of the sellers on the right, or connect Claude.
              </p>
            </div>
          ) : (
            me.tabs.map((t) => <TabReceipt key={t.channelId} tab={t} me={me} onChange={refresh} />)
          )}
        </section>

        <aside className="side">
          <div className="stub">
            <div style={{ fontSize: "var(--t-sm)", color: "#b9beb8" }}>In your wallet</div>
            <div className="big">{usd(me.wallet?.usdc ?? 0)}</div>
            <div className="perf" />
            <div className="row">
              <span>Held in tabs</span>
              <span className="mono">{usd(me.totals.escrowed)}</span>
            </div>
            <div className="row">
              <span>Paid so far</span>
              <span className="mono">{usd(me.totals.spent - me.totals.saved)}</span>
            </div>
            <div className="row">
              <span>Not yet collected</span>
              <span className="mono">{usd(me.totals.stoppable)}</span>
            </div>
            <div className="row">
              <span>Stopped</span>
              <span className="mono">{usd(me.totals.saved)}</span>
            </div>
            <div className="row">
              <span>Owner</span>
              <span className="mono">{me.wallet ? `you (${short(me.wallet.owner)})` : "…"}</span>
            </div>
            <div className="row">
              <span>Wallet</span>
              <span className="mono">
                {walletLink ? (
                  <a href={walletLink} target="_blank" rel="noreferrer" style={{ color: "inherit" }}>
                    {short(me.account.wallet)}
                  </a>
                ) : (
                  short(me.account.wallet)
                )}
              </span>
            </div>
          </div>

          <Money me={me} onChange={refresh} />

          <div className="panel">
            <h3>Connect your AI</h3>
            <p className="muted" style={{ fontSize: "var(--t-sm)", marginBottom: "0.8rem" }}>
              This link is yours alone: anyone with it can spend from this account, within your limits.
            </p>
            <CopyField value={me.mcpUrl} label="Your Izanagi link" />
            <p style={{ fontSize: "var(--t-sm)", margin: "0.9rem 0 0.4rem" }}>In Claude Code:</p>
            <CopyField value={`claude mcp add --transport http tab ${me.mcpUrl}`} label="Claude Code command" />
            <p className="muted" style={{ fontSize: "var(--t-sm)", marginTop: "0.8rem" }}>
              In Claude's settings, add it as a custom connector. Then ask: "Use Izanagi to get the latest Bitcoin price."
            </p>
          </div>

          <TryIt listings={listings} onDone={refresh} />

          <div className="panel">
            <h3>What just happened</h3>
            {feed.length === 0 ? (
              <p className="muted" style={{ fontSize: "var(--t-sm)" }}>Live. Everything your AI and your wallet do shows up here.</p>
            ) : (
              <ul className="feed">
                {feed
                  .map((i) => ({ i, d: describe(i, name) }))
                  .filter((x) => x.d)
                  .slice(0, 14)
                  .map(({ i, d }) => (
                    <li key={i.id}>
                      <time>{clock(i.at)}</time>
                      <span className={d!.stop ? "stop" : ""}>{d!.text}</span>
                    </li>
                  ))}
              </ul>
            )}
          </div>
        </aside>
      </main>
    </>
  );
}
