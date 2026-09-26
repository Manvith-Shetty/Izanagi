import { useCallback, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Footer, Header, SampleNotice } from "../components/Chrome";
import { FeedLine } from "../components/FeedLine";
import { OwnerBrake } from "../components/OwnerBrake";
import { CopyField } from "../components/CopyField";
import { ActionError, closeTab, logout, reopenTab } from "../lib/api";
import { useFeed, useMe, useNow } from "../lib/hooks";
import { clock, short, usdcShort } from "../lib/format";
import type { Approval, Tab } from "../lib/types";

/** Intercepta's toxic score against the refusal threshold. */
function Score({ score }: { score: number }) {
  const s = Math.max(0, Math.min(100, score));
  const tone = s >= 60 ? "bg-beni" : s >= 30 ? "bg-kin" : "bg-tide";
  return (
    <div className="flex items-center gap-r2" title="Intercepta toxic score. Izanagi closes the tab at 60.">
      <div className="relative h-1.5 w-20 overflow-hidden rounded-full bg-mist-deep">
        <div className={`absolute inset-0 origin-left rounded-full ${tone}`} style={{ transform: `scaleX(${s / 100})`, transition: "transform 600ms var(--ease-out-soft)" }} />
        <div className="absolute inset-y-0 left-[60%] w-px bg-sumi/40" />
      </div>
      <span className={`num text-[0.82rem] ${s >= 60 ? "font-semibold text-beni" : "text-stone"}`}>{s.toFixed(0)}</span>
    </div>
  );
}

function TabRow({
  tab,
  t,
  canAct,
  onClose,
  onReopen,
  busy,
}: {
  tab: Tab;
  t: number;
  canAct: boolean;
  onClose: (tab: Tab) => void;
  onReopen: (tab: Tab) => void;
  busy: boolean;
}) {
  const [confirming, setConfirming] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  const closed = tab.status === "closed";
  const left = tab.expiry - t;
  const live = tab.status === "open" && left > 0;

  const close = () => {
    if (!confirming) {
      setConfirming(true);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setConfirming(false), 4000);
      return;
    }
    setConfirming(false);
    onClose(tab);
  };

  return (
    <li className={`grid grid-cols-[1fr_auto] items-center gap-x-r4 gap-y-r3 px-r4 py-r4 md:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)_7.5rem_9.5rem] md:px-r5 ${closed ? "bg-beni-wash/35" : ""}`}>
      <div className="min-w-0">
        <p className="truncate font-semibold text-sumi">{tab.service ?? short(tab.seller)}</p>
        <div className="mt-r1 flex flex-wrap items-center gap-x-r3 gap-y-r1">
          {tab.service && <span className="hex text-[0.8rem] text-stone">{short(tab.seller)}</span>}
          {tab.riskScore != null && <Score score={tab.riskScore} />}
        </div>
      </div>

      <div className="text-right md:text-left">
        {closed ? (
          <p className="num text-[1.05rem] font-semibold text-beni line-through decoration-1">{usdcShort(tab.saved)} USDC</p>
        ) : (
          <p className={`num text-[1.05rem] font-semibold ${live ? "text-kin" : "text-stone"}`}>{usdcShort(tab.stoppable)} USDC</p>
        )}
        <p className="text-[0.82rem] text-stone">
          {tab.requests} paid call{tab.requests === 1 ? "" : "s"}, {usdcShort(tab.claimed)} cashed in
        </p>
      </div>

      <div className="text-[0.85rem]">
        {closed ? (
          <p className="leading-snug text-beni-deep">Closed. The seller can't collect this.</p>
        ) : live ? (
          <>
            <p className="num font-semibold text-sumi">{clock(left)}</p>
            <p className="leading-snug text-stone">left to claim</p>
          </>
        ) : (
          <p className="leading-snug text-stone">Idle. Nothing claimable.</p>
        )}
      </div>

      <div className="col-span-2 flex md:col-span-1 md:justify-end">
        {closed ? (
          <button type="button" className="btn btn-quiet w-full md:w-auto" disabled={!canAct || busy} onClick={() => onReopen(tab)}>
            Ask to reopen
          </button>
        ) : (
          <button type="button" className={`btn w-full md:w-auto ${confirming ? "btn-seal" : "btn-quiet text-beni"}`} disabled={!canAct || busy} onClick={close}>
            {busy ? "Closing" : confirming ? "Confirm close" : "Close tab"}
          </button>
        )}
      </div>
    </li>
  );
}

function PendingApproval({ a, t, name }: { a: Approval; t: number; name: (addr: string) => ReactNode }) {
  return (
    <li className="flex flex-wrap items-center gap-r3 px-r4 py-r3 md:px-r5">
      <span className="size-2 rounded-full bg-kin" aria-hidden="true" />
      <p className="min-w-0 flex-1 text-[0.93rem]">
        {a.purpose === "restore" ? "Reopen the tab with " : <>Spend up to <span className="num font-semibold">{usdcShort(a.amount)} USDC</span> with </>}
        {name(a.seller)}
        <span className="text-stone">, expires in {clock(a.expiresAt - t)}</span>
      </p>
      <Link to={`/approve/${a.id}`} className="btn btn-primary min-h-9 text-[0.88rem]">
        Review
      </Link>
    </li>
  );
}

function Figure({ label, value, tone, note }: { label: string; value: ReactNode; tone: string; note: string }) {
  return (
    <div className="px-r4 py-r4 md:px-r5">
      <p className="text-[0.85rem] text-stone">{label}</p>
      <p className={`display num mt-r1 text-[2.2rem] leading-none tracking-[-0.02em] md:text-[2.6rem] ${tone}`}>{value}</p>
      <p className="mt-r2 text-[0.85rem] leading-snug text-stone">{note}</p>
    </div>
  );
}

function SignedOut() {
  return (
    <section className="surface-raised max-w-[40rem] rounded-2xl p-r5">
      <h2 className="display text-[1.8rem] tracking-[-0.02em]">Sign in to see your tabs</h2>
      <p className="mt-r2 text-[0.95rem] text-sumi-soft">
        Your wallet and every payment your agent makes live behind your World ID. One person gets one wallet, however many agents they
        connect to it.
      </p>
      <Link to="/start" className="btn btn-primary mt-r4">
        Sign in with World ID
      </Link>
    </section>
  );
}

export default function Dashboard() {
  const query = useMe();
  const state = query.data;
  const t = useNow();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ tone: "ok" | "err"; text: string } | null>(null);

  const me = state && state.kind !== "signed_out" ? state.me : undefined;
  const sample = state?.kind === "sample";
  const canAct = state?.kind === "signed_in";
  const feed = useFeed(state?.kind === "signed_in" ? "live" : sample ? "sample" : "off");
  const firstIds = useRef<Set<number> | null>(null);
  if (feed.items.length && firstIds.current === null) firstIds.current = new Set(feed.items.map((i) => i.id));

  const tabs = useMemo(
    () => [...(me?.tabs ?? [])].sort((a, b) => Number(a.status === "closed") - Number(b.status === "closed") || a.expiry - b.expiry),
    [me],
  );
  const names = useMemo(() => new Map(tabs.filter((x) => x.service).map((x) => [x.seller.toLowerCase(), x.service])), [tabs]);
  const name = useCallback(
    (addr: string): ReactNode => {
      const l = names.get(addr.toLowerCase());
      return l ? <span className="font-semibold">{l}</span> : <span className="hex">{short(addr)}</span>;
    },
    [names],
  );

  const liveTabs = tabs.filter((x) => x.status === "open" && x.expiry > t);
  const nextLapse = liveTabs.length ? Math.min(...liveTabs.map((x) => x.expiry)) - t : null;
  const pending = (me?.approvals ?? []).filter((a) => a.status === "pending" && a.expiresAt > t);

  const run = async (seller: string, fn: () => Promise<unknown>, ok: string) => {
    setBusy(seller);
    setNotice(null);
    try {
      await fn();
      setNotice({ tone: "ok", text: ok });
      qc.invalidateQueries({ queryKey: ["me"] });
    } catch (e) {
      if (e instanceof ActionError && e.status === 401) qc.invalidateQueries({ queryKey: ["me"] });
      setNotice({ tone: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(null);
    }
  };

  const onClose = (tab: Tab) =>
    run(tab.seller, () => closeTab(tab.seller, "closed from the dashboard"), `Closed the tab with ${tab.service ?? short(tab.seller)}. What it signed and hasn't cashed in can no longer be claimed.`);
  const onReopen = (tab: Tab) =>
    run(
      tab.seller,
      async () => {
        const r = await reopenTab(tab.seller);
        navigate(`/approve/${r.id}`);
      },
      "Sent the request to World App.",
    );
  const signOut = async () => {
    await logout().catch(() => undefined);
    qc.invalidateQueries({ queryKey: ["me"] });
  };

  return (
    <div className="ground grain min-h-dvh">
      <SampleNotice show={sample} />
      <Header>
        {canAct && (
          <button type="button" className="btn btn-quiet min-h-10 px-r4 text-[0.88rem]" onClick={signOut}>
            Sign out
          </button>
        )}
      </Header>

      <main className="mx-auto max-w-[76rem] px-r4 pb-r7 md:px-r5">
        <div className="flex flex-col gap-r3 pb-r5 pt-r4 md:flex-row md:items-end">
          <div>
            <h1 className="display text-[2.5rem] md:text-[3.2rem]">Your agent's tabs</h1>
            {me && (
              <p className="mt-r2 text-[0.93rem] text-sumi-soft">
                Wallet <span className="hex text-sumi">{short(me.account.wallet, 8, 6)}</span> on {me.network.fork ? "a local Base fork" : "Base"}
                {me.wallet && (
                  <>
                    , holding <span className="num text-sumi">{usdcShort(me.wallet.usdc)} USDC</span>
                  </>
                )}
                . Approvals go to one verified person.
              </p>
            )}
          </div>
        </div>

        {query.isLoading ? (
          <p className="text-stone">Loading your tabs…</p>
        ) : state?.kind === "signed_out" ? (
          <SignedOut />
        ) : me ? (
          <>
            <section aria-label="Summary" className="surface-raised grid divide-y divide-rule/70 rounded-2xl md:grid-cols-3 md:divide-x md:divide-y-0">
              <Figure label="Signed, not yet claimed" value={usdcShort(me.totals.stoppable)} tone="text-kin" note="USDC your agent has signed for that you can still stop." />
              <Figure label="Stopped after signing" value={usdcShort(me.totals.saved)} tone="text-beni" note="USDC signed for on tabs you closed, that the seller can never collect." />
              <Figure
                label="Next claim window closes in"
                value={nextLapse === null ? "None open" : clock(nextLapse)}
                tone="text-sumi"
                note="Sellers must claim before then. After it, unclaimed vouchers lapse by themselves."
              />
            </section>

            <div className="mt-r5 grid gap-r5 lg:grid-cols-12">
              <div className="flex flex-col gap-r5 lg:col-span-7">
                {pending.length > 0 && (
                  <section aria-labelledby="approvals" className="surface-raised overflow-hidden rounded-2xl ring-1 ring-kin/30">
                    <h2 id="approvals" className="border-b border-rule/70 px-r4 py-r3 font-semibold md:px-r5">
                      Waiting for you
                    </h2>
                    <ul className="divide-y divide-rule/60">
                      {pending.map((a) => (
                        <PendingApproval key={a.id} a={a} t={t} name={name} />
                      ))}
                    </ul>
                  </section>
                )}

                <section aria-labelledby="tabs" className="surface-raised overflow-hidden rounded-2xl">
                  <div className="flex items-baseline gap-r3 border-b border-rule/70 px-r4 py-r3 md:px-r5">
                    <h2 id="tabs" className="font-semibold">
                      Open tabs
                    </h2>
                    <span className="text-[0.85rem] text-stone">{liveTabs.length} live</span>
                  </div>
                  {tabs.length ? (
                    <ul className="divide-y divide-rule/60">
                      {tabs.map((x) => (
                        <TabRow key={x.channelId} tab={x} t={t} canAct={canAct} busy={busy === x.seller} onClose={onClose} onReopen={onReopen} />
                      ))}
                    </ul>
                  ) : (
                    <div className="px-r5 py-r5">
                      <p className="font-semibold">No tabs yet</p>
                      <p className="mt-r1 max-w-[30rem] text-[0.93rem] text-sumi-soft">
                        A tab opens the first time your agent pays a seller. Connect an agent through the MCP server below and ask it to buy
                        something.
                      </p>
                    </div>
                  )}
                  <div aria-live="polite">
                    {notice && (
                      <p className={`border-t border-rule/70 px-r4 py-r3 text-[0.9rem] md:px-r5 ${notice.tone === "err" ? "text-beni-deep" : "text-tide"}`}>{notice.text}</p>
                    )}
                  </div>
                </section>

                <OwnerBrake wallet={me.account.wallet} sample={sample} />
              </div>

              <aside className="flex flex-col gap-r5 lg:col-span-5">
                <section aria-labelledby="live" className="surface-raised overflow-hidden rounded-2xl">
                  <div className="flex items-center gap-r3 border-b border-rule/70 px-r4 py-r3 md:px-r5">
                    <h2 id="live" className="font-semibold">
                      What happened
                    </h2>
                    <span className="ml-auto flex items-center gap-r2 text-[0.82rem] text-stone">
                      <span className={`size-2 rounded-full ${feed.live ? "bg-tide animate-[breathe_2.4s_ease-in-out_infinite]" : "bg-stone-light"}`} aria-hidden="true" />
                      {feed.live ? "Live" : feed.sample ? "Sample" : "Reconnecting"}
                    </span>
                  </div>
                  {feed.items.length ? (
                    <ol className="max-h-[38rem] divide-y divide-rule/50 overflow-y-auto px-r4 md:px-r5">
                      {feed.items.map((i) => (
                        <FeedLine key={i.id} item={i} name={name} fresh={!!firstIds.current && !firstIds.current.has(i.id)} />
                      ))}
                    </ol>
                  ) : (
                    <p className="px-r5 py-r5 text-[0.93rem] text-sumi-soft">Nothing yet. Every screening, signature and closed tab shows up here as it happens.</p>
                  )}
                </section>
                <CopyField label="Connect an agent: MCP server URL" value={me.mcpUrl} />
              </aside>
            </div>
          </>
        ) : null}
      </main>
      <Footer />
    </div>
  );
}
