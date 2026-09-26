import { useState } from "react";
import { api, clock, short, usd, type Me } from "../api";
import { connect, explain, send } from "../metamask";

/** Dollars typed by a person -> atomic USDC, or undefined if it isn't an amount. */
function atomic(dollars: string): number | undefined {
  const n = Number(dollars.trim().replace(/^\$/, ""));
  if (!Number.isFinite(n) || n <= 0) return undefined;
  return Math.round(n * 1_000_000);
}

/**
 * Adding money, and taking it out. A free trial is Tab's wallet; the first deposit from the
 * person's MetaMask makes it theirs, and from then on only that account can take money out.
 */
export function Money({ me, onChange }: { me: Me; onChange: () => void }) {
  const [amount, setAmount] = useState("5");
  const [step, setStep] = useState<string>();
  const [done, setDone] = useState<string>();
  /** A handover waiting on the person's World ID, and the deposit that asked for it. */
  const [pending, setPending] = useState<{ approval: string; tx: string }>();
  const [error, setError] = useState<string>();
  const yours = me.ownership?.yours ?? false;
  const owner = me.ownership?.owner;

  const run = async (work: () => Promise<string>) => {
    setError(undefined);
    setDone(undefined);
    setPending(undefined);
    try {
      setDone(await work());
      onChange();
    } catch (e) {
      setError(explain(e));
    } finally {
      setStep(undefined);
    }
  };

  const add = () =>
    run(async () => {
      const value = atomic(amount);
      if (!value) throw new Error("Type an amount in dollars, like 5.");
      setStep("Connecting MetaMask…");
      const { tx, chainId } = await api.depositTx(value);
      const from = await connect(chainId);
      if (yours && owner && from !== owner.toLowerCase()) {
        throw new Error(`This wallet belongs to ${short(owner)}. Switch MetaMask to that account to add money.`);
      }
      setStep(`Confirm sending ${usd(value)} in MetaMask…`);
      const hash = await send(from, tx);
      return confirm(hash);
    });

  /** Tab checks the deposit; a trial wallet then waits on the person's World ID to become theirs. */
  const confirm = async (hash: string) => {
    setStep("Waiting for it to land on Base…");
    const r = await api.deposited(hash);
    if (r.approval) {
      setPending({ approval: r.approval.id, tx: hash });
      return `Added ${usd(r.deposit.amount)}. One last step: approve with World ID so that ${short(r.deposit.from)} becomes the wallet's owner.`;
    }
    return `Added ${usd(r.deposit.amount)}.`;
  };

  const takeOut = () =>
    run(async () => {
      setStep("Working out what to send back…");
      const { chainId, plan } = await api.withdrawPlan();
      if (plan.txs.length === 0) {
        if (plan.readyAt) return `Your tabs are on their way back. Come back after ${clock(plan.readyAt)} to finish.`;
        return "There's nothing to take out.";
      }
      setStep("Connecting MetaMask…");
      const from = await connect(chainId);
      if (from !== plan.owner.toLowerCase()) {
        throw new Error(`Only ${short(plan.owner)} can take money out. Switch MetaMask to that account.`);
      }
      const started = plan.txs.some((t) => t.label.startsWith("Start"));
      for (const [i, tx] of plan.txs.entries()) {
        setStep(`${i + 1} of ${plan.txs.length}: ${tx.label}. Confirm in MetaMask…`);
        await send(from, tx);
      }
      return started
        ? "Done for now. Sellers get a waiting period to collect what they're owed; come back after it to take back what's left in your tabs."
        : "Done. The money is on its way to your MetaMask.";
    });

  return (
    <div className="panel">
      <h3>{yours ? "Your money" : "Make it yours"}</h3>
      {yours ? (
        <p className="muted" style={{ fontSize: "var(--t-sm)", marginBottom: "0.8rem" }}>
          Owned by <span className="mono">{short(owner!)}</span>, your MetaMask. Only that account can take money out. Your AI
          can spend within your limits, but never withdraw.
        </p>
      ) : (
        <p className="muted" style={{ fontSize: "var(--t-sm)", marginBottom: "0.8rem" }}>
          Tab holds the keys to this free trial wallet. Add money from MetaMask and the wallet becomes yours: after that only
          your MetaMask can take money out, not Tab and not your AI.
        </p>
      )}
      <div className="amount-row">
        <label className="amount">
          <span>$</span>
          <input inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} aria-label="Amount in US dollars" disabled={!!step} />
        </label>
        <button className="btn small" onClick={add} disabled={!!step}>
          {yours ? "Add money" : "Connect MetaMask & add"}
        </button>
        {yours && (
          <button className="btn small ghost" onClick={takeOut} disabled={!!step}>
            Take money out
          </button>
        )}
      </div>
      <p className="muted" style={{ fontSize: "var(--t-xs)", marginTop: "0.6rem" }}>
        USDC on Base. MetaMask needs a little ETH on Base for the fee, usually under a cent.
      </p>
      {step && <div className="notice" style={{ marginTop: "0.8rem" }}>{step}</div>}
      {done && (
        <div className="notice" style={{ marginTop: "0.8rem" }}>
          {done}
          {pending && !yours && (
            <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap", marginTop: "0.6rem" }}>
              <a className="btn small" href={`/approve/${pending.approval}`} target="_blank" rel="noreferrer">
                Approve with World ID
              </a>
              <button className="btn small ghost" onClick={() => run(() => confirm(pending.tx))}>
                Ask again
              </button>
            </div>
          )}
        </div>
      )}
      {error && <div className="error" style={{ marginTop: "0.8rem" }}>{error}</div>}
    </div>
  );
}
