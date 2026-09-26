import { useCallback, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Footer, Header, SampleNotice } from "../components/Chrome";
import { FeedLine } from "../components/FeedLine";
import { OwnerBrake } from "../components/OwnerBrake";
import { AdminKey } from "../components/AdminKey";
import { CopyField } from "../components/CopyField";
import { ActionError, adminToken, closeTab, reopenTab, setAdminToken } from "../lib/api";
import { useFeed, useNow, useOverview } from "../lib/hooks";
import { clock, short, usdcShort, words } from "../lib/format";
import type { Approval, Session } from "../lib/types";

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
  s,
  t,
  canAct,
  onClose,
  onReopen,
  busy,
}: {
  s: Session;
  t: number;
  canAct: boolean;
  onClose: (s: Session) => void;
  onReopen: (s: Session) => void;
  busy: boolean;
}) {
  const [confirming, setConfirming] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  const left = s.expiry - t;
  const live = !s.revoked && left > 0;

  const close = () => {
    if (!confirming) {
      setConfirming(true);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setConfirming(false), 4000);
      return;
    }
    setConfirming(false);
    onClose(s);
  };

  return (
    <li className={`grid grid-cols-[1fr_auto] items-center gap-x-r4 gap-y-r3 px-r4 py-r4 md:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)_7.5rem_9.5rem] md:px-r5 ${s.revoked ? "bg-beni-wash/35" : ""}`}>
      <div className="min-w-0">
        <p className="truncate font-semibold text-sumi">{s.label ?? short(s.seller)}</p>
        <div className="mt-r1 flex flex-wrap items-center gap-x-r3 gap-y-r1">
          {s.label && <span className="hex text-[0.8rem] text-stone">{short(s.seller)}</span>}
          <Score score={s.last_score} />
        </div>
      </div>

      <div className="text-right md:text-left">
        <p className={`num text-[1.05rem] font-semibold ${s.revoked ? "text-beni line-through decoration-1" : "text-kin"}`}>{usdcShort(s.ceiling)} USDC</p>
        <p className="text-[0.82rem] text-stone">
          {s.vouchers} voucher{s.vouchers === 1 ? "" : "s"}
        </p>
      </div>

      <div className="text-[0.85rem]">
        {s.revoked ? (
          <p className="leading-snug text-beni-deep">Closed: {words(s.revoked_reason ?? "by you")}</p>
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
        {s.revoked ? (
          <button type="button" className="btn btn-quiet w-full md:w-auto" disabled={!canAct || busy} onClick={() => onReopen(s)}>
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

function PendingApproval({ a, t }: { a: Approval; t: number }) {
  return (
    <li className="flex flex-wrap items-center gap-r3 px-r4 py-r3 md:px-r5">
      <span className="size-2 rounded-full bg-kin" aria-hidden="true" />
      <p className="min-w-0 flex-1 text-[0.93rem]">
        {a.purpose === "restore" ? "Reopen the tab with " : <>Pay <span className="num font-semibold">{usdcShort(a.amount)} USDC</span> to </>}
        <span className="hex">{short(a.seller)}</span>
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

export default function Dashboard() {
  const overview = useOverview();
  const feed = useFeed();
  const t = useNow();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [askKey, setAskKey] = useState(false);
  const [pending, setPending] = useState<null | (() => void)>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const firstIds = useRef<Set<number> | null>(null);

  const o = overview.data?.data;
  const sample = overview.data?.sample ?? false;
  const sessions = useMemo(() => [...(o?.sessions ?? [])].sort((a, b) => Number(a.revoked) - Number(b.revoked) || a.expiry - b.expiry), [o]);

  const names = useMemo(() => new Map(sessions.map((s) => [s.seller.toLowerCase(), s.label])), [sessions]);
  const name = useCallback(
    (addr: string): ReactNode => {
      const l = names.get(addr.toLowerCase());
      return l ? <span className="font-semibold">{l}</span> : <span className="hex">{short(addr)}</span>;
    },
    [names],
  );

  if (feed.items.length && firstIds.current === null) firstIds.current = new Set(feed.items.map((i) => i.id));

  const liveSessions = sessions.filter((s) => !s.revoked && s.expiry > t);
  const stoppable = liveSessions.reduce((n, s) => n + Number(s.ceiling), 0);
  const stopped = sessions.filter((s) => s.revoked);
  const stoppedValue = stopped.reduce((n, s) => n + Number(s.ceiling), 0);
  const stoppedVouchers = stopped.reduce((n, s) => n + s.vouchers, 0);
  const nextLapse = liveSessions.length ? Math.min(...liveSessions.map((s) => s.expiry)) - t : null;
  const canAct = !!o && !o.readOnly && !sample;

  const withKey = (fn: () => void) => {
    if (adminToken()) fn();
    else {
      setPending(() => fn);
      setAskKey(true);
    }
  };

  const run = async (seller: string, fn: () => Promise<unknown>, ok: string) => {
    setBusy(seller);
    setNotice(null);
    try {
      await fn();
      setNotice({ tone: "ok", text: ok });
      qc.invalidateQueries({ queryKey: ["overview"] });
    } catch (e) {
      if (e instanceof ActionError && (e.status === 401 || e.status === 403)) {
        setAdminToken(null);
        setPending(() => () => run(seller, fn, ok));
        setAskKey(true);
      }
      setNotice({ tone: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(null);
    }
  };

  const onClose = (s: Session) =>
    withKey(() => run(s.seller, () => closeTab(s.seller, "operator"), `Closed the tab with ${s.label ?? short(s.seller)}. Its signed vouchers can no longer be claimed.`));
  const onReopen = (s: Session) =>
    withKey(() =>
      run(
        s.seller,
        async () => {
          const r = await reopenTab(s.seller);
          navigate(`/approve/${r.approvalId}`);
        },
        "Sent the request to your phone.",
      ),
    );

  return (
    <div className="ground grain min-h-dvh">
      <SampleNotice show={sample} />
      <Header>
        {o && !o.readOnly && !sample && (
          <button type="button" className="btn btn-quiet min-h-10 px-r4 text-[0.88rem]" onClick={() => setAskKey(true)}>
            {adminToken() ? "Admin token set" : "Unlock actions"}
          </button>
        )}
      </Header>

      <main className="mx-auto max-w-[76rem] px-r4 pb-r7 md:px-r5">
        <div className="flex flex-col gap-r3 pb-r5 pt-r4 md:flex-row md:items-end">
          <div>
            <h1 className="display text-[2.5rem] md:text-[3.2rem]">Your agent's tabs</h1>
            <p className="mt-r2 text-[0.93rem] text-sumi-soft">
              Wallet <span className="hex text-sumi">{o ? short(o.wallet, 8, 6) : "…"}</span>
              {o && <> on {o.fork ? "a local Base fork" : "Base"}</>}
              {o?.human && <>. Approvals go to one verified person</>}
            </p>
          </div>
          {o?.readOnly && !sample && <p className="text-[0.88rem] text-stone md:ml-auto">Read-only: Tab has no admin token set.</p>}
        </div>

        <section aria-label="Summary" className="surface-raised grid divide-y divide-rule/70 rounded-2xl md:grid-cols-3 md:divide-x md:divide-y-0">
          <Figure label="Signed, not yet claimed" value={`${usdcShort(stoppable)}`} tone="text-kin" note="USDC your agent has signed for that you can still stop." />
          <Figure
            label="Stopped after signing"
            value={`${usdcShort(stoppedValue)}`}
            tone="text-beni"
            note={`USDC in ${stoppedVouchers} signed voucher${stoppedVouchers === 1 ? "" : "s"} that will never be paid.`}
          />
          <Figure
            label="Next claim window closes in"
            value={nextLapse === null ? "None open" : clock(nextLapse)}
            tone="text-sumi"
            note="Sellers must claim before then. After it, unclaimed vouchers lapse by themselves."
          />
        </section>

        <div className="mt-r5 grid gap-r5 lg:grid-cols-12">
          <div className="flex flex-col gap-r5 lg:col-span-7">
            {!!o?.approvals.length && (
              <section aria-labelledby="approvals" className="surface-raised overflow-hidden rounded-2xl ring-1 ring-kin/30">
                <h2 id="approvals" className="border-b border-rule/70 px-r4 py-r3 font-semibold md:px-r5">
                  Waiting for you
                </h2>
                <ul className="divide-y divide-rule/60">
                  {o.approvals.map((a) => (
                    <PendingApproval key={a.id} a={a} t={t} />
                  ))}
                </ul>
              </section>
            )}

            <section aria-labelledby="tabs" className="surface-raised overflow-hidden rounded-2xl">
              <div className="flex items-baseline gap-r3 border-b border-rule/70 px-r4 py-r3 md:px-r5">
                <h2 id="tabs" className="font-semibold">
                  Open tabs
                </h2>
                <span className="text-[0.85rem] text-stone">{liveSessions.length} live</span>
              </div>
              {overview.isLoading ? (
                <p className="px-r5 py-r5 text-stone">Loading your tabs…</p>
              ) : sessions.length ? (
                <ul className="divide-y divide-rule/60">
                  {sessions.map((s) => (
                    <TabRow key={s.channel_id} s={s} t={t} canAct={canAct} busy={busy === s.seller} onClose={onClose} onReopen={onReopen} />
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

            {o && <OwnerBrake wallet={o.wallet} sample={sample} />}
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
            {o && <CopyField label="Connect an agent: MCP server URL" value={o.mcpUrl} />}
          </aside>
        </div>
      </main>
      <Footer />

      <AdminKey
        open={askKey}
        onClose={() => {
          setAskKey(false);
          setPending(null);
        }}
        onSaved={() => {
          setAskKey(false);
          const p = pending;
          setPending(null);
          p?.();
        }}
      />
    </div>
  );
}
