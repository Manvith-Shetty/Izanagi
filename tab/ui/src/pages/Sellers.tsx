import { Footer, Header } from "../components/Chrome";
import { Link } from "react-router-dom";

const changes = [
  {
    h: "You're paid the same way",
    p: "Agents pay you through Coinbase's x402 batch-settlement escrow on Base, with the same terms, headers and vouchers as any other payer. Nothing to integrate.",
  },
  {
    h: "Claim inside the window",
    p: "Each voucher stays claimable for two minutes after the agent's last request. Read the expiry from the countersignature and claim before it lapses. The reference seller does this for you.",
  },
  {
    h: "Stay clean, stay paid",
    p: "If Intercepta flags your address, the payer's wallet stops honouring your unclaimed vouchers. Honest sellers never notice; a drained key or a hijacked endpoint can't cash out.",
  },
];

const credit = [
  { score: "Below 30", served: "Yes", carried: "Up to 1 USDC unclaimed" },
  { score: "30 to 59", served: "Yes", carried: "None. Claim after every request" },
  { score: "60 and above", served: "No, refused with 403", carried: "None" },
];

export default function Sellers() {
  return (
    <div className="ground grain min-h-dvh">
      <Header>
        <Link to="/dashboard" className="btn btn-primary min-h-10 px-r4 text-[0.9rem]">
          Open dashboard
        </Link>
      </Header>
      <main>
        <section className="mx-auto max-w-[76rem] px-r4 pb-r6 pt-r5 md:px-r5 md:pt-r6">
          <h1 className="display max-w-[17ch] text-[2.7rem] sm:text-[3.6rem] lg:text-[4.4rem]">Sell to agents whose owners can still say no</h1>
          <p className="mt-r5 max-w-[40rem] text-[1.1rem] text-sumi-soft">
            Agents on Izanagi wallets pay you in USDC over x402, like any other agent. The difference is that their owners can stop a payment
            until you claim it. Here's what that means for you.
          </p>
          <div className="mt-r6 grid gap-r5 border-t border-rule/80 pt-r5 md:grid-cols-3">
            {changes.map((c) => (
              <div key={c.h}>
                <h2 className="display text-[1.45rem] tracking-[-0.02em]">{c.h}</h2>
                <p className="mt-r2 text-[0.97rem] text-sumi-soft">{c.p}</p>
              </div>
            ))}
          </div>
        </section>

        <section className="border-t border-rule/80 bg-paper/60">
          <div className="mx-auto grid max-w-[76rem] gap-r6 px-r4 py-r7 md:px-r5 lg:grid-cols-12">
            <div className="lg:col-span-5">
              <h2 className="display text-[2.2rem] md:text-[2.6rem]">Every request you serve is credit. Screen who you extend it to.</h2>
              <p className="mt-r4 max-w-[32rem] text-sumi-soft">
                With batch settlement you serve first and get paid later. The reference seller screens each payer through Intercepta and turns
                the score into a credit line, not a yes or no. Before serving, it asks the payer's wallet the same question the escrow will
                ask at claim time, so a revoked seller finds out at the door.
              </p>
            </div>
            <div className="lg:col-span-7">
              <div className="surface-raised overflow-x-auto rounded-2xl">
                <table className="w-full min-w-[30rem] text-left text-[0.95rem]">
                  <caption className="sr-only">How the reference seller treats a payer, by Intercepta score</caption>
                  <thead>
                    <tr className="border-b border-rule text-[0.82rem] text-stone">
                      <th scope="col" className="px-r4 py-r3 font-medium">
                        Payer's toxic score
                      </th>
                      <th scope="col" className="px-r4 py-r3 font-medium">
                        Served?
                      </th>
                      <th scope="col" className="px-r4 py-r3 font-medium">
                        Unpaid value you carry
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {credit.map((r, i) => (
                      <tr key={r.score} className={`border-b border-rule/60 last:border-0 ${i === 2 ? "bg-beni-wash/40" : ""}`}>
                        <th scope="row" className="num px-r4 py-r3 font-semibold">
                          {r.score}
                        </th>
                        <td className={`px-r4 py-r3 ${i === 2 ? "font-semibold text-beni" : "text-sumi-soft"}`}>{r.served}</td>
                        <td className="px-r4 py-r3 text-sumi-soft">{r.carried}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="mt-r4 text-[0.88rem] text-stone">What your logs say when a payer revoked you after you served them:</p>
              <pre className="hex mt-r2 overflow-x-auto rounded-2xl bg-sumi p-r4 text-[0.82rem] leading-relaxed text-mist/90" style={{ boxShadow: "var(--shadow-float)" }}>
                {`CLAIM REJECTED by the escrow: 0.020000 USDC we served for is unclaimable.
the payer's wallet revoked this seller at 1790424567 (reason: wallet_drainer)
- after we had served the requests`}
              </pre>
            </div>
          </div>
        </section>

        <section className="mx-auto max-w-[76rem] px-r4 py-r7 md:px-r5">
          <h2 className="display max-w-[24ch] text-[2.2rem] md:text-[2.6rem]">Run the reference seller</h2>
          <p className="mt-r4 max-w-[40rem] text-sumi-soft">
            It serves x402 batch-settlement requests, screens payers, checks each voucher against the payer's wallet, and claims before the
            window closes. Configure it in <span className="hex">seller/.env</span>, then start it:
          </p>
          <pre className="hex surface-raised mt-r4 inline-block rounded-xl px-r4 py-r3 text-[0.9rem]">cargo run -p seller</pre>
        </section>
      </main>
      <Footer />
    </div>
  );
}
