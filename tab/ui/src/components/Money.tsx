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
 * Adding money, and taking it out. The wallet belongs to the person's MetaMask account from the
 * block that created it: any account may add money, only that one can take money out.
 */
export function Money({ me, onChange }: { me: Me; onChange: () => void }) {
  const [amount, setAmount] = useState("5");
  const [step, setStep] = useState<string>();
  const [done, setDone] = useState<string>();
  const [error, setError] = useState<string>();
  const owner = me.wallet?.owner;

  const run = async (work: () => Promise<string>) => {
    setError(undefined);
    setDone(undefined);
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
      setStep(`Confirm sending ${usd(value)} in MetaMask…`);
      const hash = await send(from, tx);
      setStep("Waiting for it to land on Base…");
      const r = await api.deposited(hash);
      return `Added ${usd(r.deposit.amount)}.`;
    });

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
      <h3>Your money</h3>
      <p className="muted" style={{ fontSize: "var(--t-sm)", marginBottom: "0.8rem" }}>
        {owner ? (
          <>
            Owned by <span className="mono">{short(owner)}</span>, your MetaMask.{" "}
          </>
        ) : null}
        Only that account can take money out. Your AI can spend within your limits, but never withdraw.
      </p>
      <div className="amount-row">
        <label className="amount">
          <span>$</span>
          <input inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} aria-label="Amount in US dollars" disabled={!!step} />
        </label>
        <button className="btn small" onClick={add} disabled={!!step}>
          Add money
        </button>
        <button className="btn small ghost" onClick={takeOut} disabled={!!step}>
          Take money out
        </button>
      </div>
      <p className="muted" style={{ fontSize: "var(--t-xs)", marginTop: "0.6rem" }}>
        USDC on Base. MetaMask needs a little ETH on Base for the fee, usually under a cent.
      </p>
      {step && <div className="notice" style={{ marginTop: "0.8rem" }}>{step}</div>}
      {done && <div className="notice" style={{ marginTop: "0.8rem" }}>{done}</div>}
      {error && <div className="error" style={{ marginTop: "0.8rem" }}>{error}</div>}
    </div>
  );
}
