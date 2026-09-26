import { Link } from "react-router-dom";
import { Footer, Header } from "../components/Chrome";
import { Slope } from "../components/Slope";
import { CopyField } from "../components/CopyField";
import { useCensus, useOverview } from "../lib/hooks";
import { usdc } from "../lib/format";

function Census() {
  const { data } = useCensus();
  if (!data) return <p className="h-[3.4rem]" />;
  const c = data.data;
  return (
    <p className="max-w-[46rem] text-[1.08rem] leading-[1.7] text-sumi-soft md:text-[1.18rem]">
      {c.inEscrow !== undefined && (
        <>
          Right now <span className="num font-semibold text-sumi">{usdc(c.inEscrow)} USDC</span> sits in x402 escrow on Base.{" "}
        </>
      )}
      In the last {c.windowDays} days, <span className="num font-semibold text-sumi">{c.channels}</span> agent payment channels opened
      there, from {c.payers} wallets. <span className="num font-semibold text-sumi">{c.singleKey} of {c.channels}</span> are
      controlled by a single key, and <span className="num font-semibold text-beni">{c.withPolicy}</span> can be stopped once a payment
      is signed.
      {data.sample && <span className="text-stone"> Measured on Base mainnet, 26 September 2026.</span>}
    </p>
  );
}

const checks: [string, boolean][] = [
  ["Your agent signed this exact voucher", true],
  ["Intercepta screened the seller and Izanagi countersigned", true],
  ["That countersignature hasn't expired", true],
  ["You haven't revoked the seller since", false],
  ["You haven't paused the wallet since", false],
];

const verdicts = [
  { v: "Pay", tone: "text-tide bg-tide-wash", d: "The seller is clean and the amount is within your limits. Izanagi countersigns." },
  { v: "Cap", tone: "text-kin bg-kin-wash", d: "The seller is fine but the amount isn't. The agent can pay up to your cap and no more." },
  { v: "Ask", tone: "text-sumi bg-mist-deep", d: "Over your limit. Nothing is signed until you approve it with World ID." },
  { v: "Refuse", tone: "text-beni bg-beni-wash", d: "A risky seller. No voucher is countersigned, so no payment can ever be claimed." },
];

type Cell = "yes" | "no" | "after";
const trust: { who: string; cells: Cell[] }[] = [
  { who: "Move money", cells: ["no", "no", "yes", "no"] },
  { who: "Send it to another seller", cells: ["no", "no", "yes", "no"] },
  { who: "Block a payment", cells: ["no", "yes", "yes", "no"] },
  { who: "Get the money back", cells: ["no", "no", "yes", "after"] },
];

function Mark({ c }: { c: Cell }) {
  if (c === "yes") return <span className="font-semibold text-tide">Yes</span>;
  if (c === "after") return <span className="text-sumi">Yes, after 15 min</span>;
  return <span className="text-stone-light">No</span>;
}

export default function Landing() {
  const overview = useOverview();
  const mcpUrl = overview.data?.data.mcpUrl ?? `${location.origin}/mcp`;
  const claudeConfig = JSON.stringify({ mcpServers: { izanagi: { type: "http", url: mcpUrl } } }, null, 2);

  return (
    <div className="ground grain min-h-dvh">
      <Header>
        <Link to="/dashboard" className="btn btn-primary min-h-10 px-r4 text-[0.9rem]">
          Open dashboard
        </Link>
      </Header>

      <main>
        {/* hero */}
        <section className="mx-auto max-w-[76rem] px-r4 pb-r6 pt-r5 md:px-r5 md:pt-r6">
          <h1 className="display text-[2.9rem] text-sumi sm:text-[4rem] lg:text-[5.4rem]">
            <span className="block">Your agent already paid.</span>
            <span className="block">You can still take it back.</span>
          </h1>
          <p className="mt-r5 max-w-[40rem] text-[1.1rem] text-sumi-soft md:text-[1.2rem]">
            Izanagi is a wallet for AI agents. It checks every payment again at the moment the seller tries to cash it in, so a seller
            that turns bad after your agent has paid still gets nothing.
          </p>
          <div className="mt-r6">
            <Slope />
          </div>
          <div className="mt-r5">
            <Census />
          </div>
        </section>

        {/* where the check happens */}
        <section id="how" className="scroll-mt-r5 border-t border-rule/80 bg-paper/60">
          <div className="mx-auto grid max-w-[76rem] gap-r6 px-r4 py-r7 md:px-r5 lg:grid-cols-12">
            <div className="lg:col-span-5">
              <h2 className="display text-[2.2rem] md:text-[2.8rem]">Other guards check before the agent signs. We check again at the claim.</h2>
              <p className="mt-r4 max-w-[34rem] text-sumi-soft">
                With most agent wallets, signing is spending: once the signature exists, the money is gone. Izanagi runs on x402 batch
                settlement, where the agent signs vouchers as it goes and the seller claims them later, through Coinbase's escrow.
              </p>
              <p className="mt-r3 max-w-[34rem] text-sumi-soft">
                At claim time the escrow asks the Izanagi wallet whether each voucher is still valid. Five things have to hold. The
                last two are read live, when the seller claims.
              </p>
            </div>
            <div className="lg:col-span-7">
              <div className="surface-raised overflow-hidden rounded-2xl">
                <table className="w-full text-left text-[0.95rem]">
                  <caption className="sr-only">What must hold for a seller to claim a voucher</caption>
                  <thead>
                    <tr className="border-b border-rule text-[0.82rem] text-stone">
                      <th scope="col" className="px-r4 py-r3 font-medium">
                        A voucher can be claimed only if
                      </th>
                      <th scope="col" className="w-[9.5rem] px-r4 py-r3 font-medium">
                        Checkable before signing?
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {checks.map(([c, before]) => (
                      <tr key={c} className={`border-b border-rule/60 last:border-0 ${before ? "" : "bg-beni-wash/40"}`}>
                        <td className={`px-r4 py-r3 ${before ? "text-sumi-soft" : "font-semibold text-sumi"}`}>{c}</td>
                        <td className="px-r4 py-r3">{before ? <span className="text-stone">Yes</span> : <span className="font-semibold text-beni">No</span>}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="mt-r3 text-[0.88rem] text-stone">
                The escrow is Coinbase's deployed <span className="hex">x402BatchSettlement</span> contract on Base. Izanagi didn't write
                it, which is why the guarantee holds.
              </p>
            </div>
          </div>
        </section>

        {/* the two gates */}
        <section className="mx-auto max-w-[76rem] px-r4 py-r7 md:px-r5">
          <h2 className="display max-w-[22ch] text-[2.2rem] md:text-[2.8rem]">Every voucher is screened. Anything unusual goes to a person.</h2>
          <div className="mt-r6 grid gap-r6 lg:grid-cols-2">
            <div>
              <h3 className="display text-[1.5rem] tracking-[-0.02em]">Intercepta decides before Izanagi signs</h3>
              <p className="mt-r3 max-w-[34rem] text-sumi-soft">
                Izanagi screens the seller, the token and the payment itself through Intercepta before countersigning. Without a verdict
                there is no countersignature, and without one no voucher can be claimed. If Intercepta is down, Izanagi refuses.
              </p>
              <ul className="mt-r4 divide-y divide-rule/70 border-y border-rule/70">
                {verdicts.map((v) => (
                  <li key={v.v} className="flex items-baseline gap-r4 py-r3">
                    <span className={`w-[4.5rem] shrink-0 rounded-md px-r2 py-[0.15rem] text-center text-[0.85rem] font-semibold ${v.tone}`}>{v.v}</span>
                    <span className="text-[0.95rem] text-sumi-soft">{v.d}</span>
                  </li>
                ))}
              </ul>
              <p className="mt-r3 text-[0.9rem] text-stone">
                Scores keep moving after a payment. Izanagi re-screens every open tab, and when a seller's score crosses your threshold it
                revokes them, voiding vouchers that were already signed.
              </p>
            </div>
            <div>
              <h3 className="display text-[1.5rem] tracking-[-0.02em]">You approve the rest with World ID</h3>
              <p className="mt-r3 max-w-[34rem] text-sumi-soft">
                When a payment is over your limit, or you want a closed tab reopened, Izanagi sends the request to your phone. Nothing is
                signed until a verified person says yes.
              </p>
              <div className="surface-raised mt-r4 rounded-2xl p-r4">
                <p className="text-[0.82rem] text-stone">Your agent is asking to pay</p>
                <p className="display num mt-r1 text-[2.3rem] leading-none">0.15 USDC</p>
                <p className="mt-r2 text-[0.95rem] text-sumi-soft">
                  to <span className="font-semibold text-sumi">onesource</span>, over the 0.10 USDC per-call limit you set.
                </p>
                <div className="mt-r4 flex flex-wrap items-center gap-r3">
                  <span className="hex rounded-lg bg-mist-deep px-r3 py-r2 text-[1.05rem] font-medium tracking-[0.12em]">KQXT-MRWD</span>
                  <span className="text-[0.88rem] text-stone">Enter this code in World App</span>
                </div>
              </div>
              <p className="mt-r3 text-[0.9rem] text-stone">
                One approval covers one payment: this amount, to this seller. It can't be reused for another. Denied, expired or
                cancelled requests sign nothing.
              </p>
            </div>
          </div>
        </section>

        {/* trust */}
        <section className="border-y border-rule/80 bg-sumi text-mist">
          <div className="mx-auto grid max-w-[76rem] gap-r6 px-r4 py-r7 md:px-r5 lg:grid-cols-12">
            <div className="lg:col-span-5">
              <h2 className="display text-[2.2rem] text-paper md:text-[2.8rem]">Izanagi can say no. It can't take your money.</h2>
              <p className="mt-r4 max-w-[32rem] text-mist/75">
                Two keys guard the wallet: your agent's, and Izanagi's countersigning key. Neither can move funds on its own. If Izanagi
                disappears, you withdraw from the escrow yourself after the 15-minute delay.
              </p>
            </div>
            <div className="overflow-x-auto lg:col-span-7">
              <table className="w-full min-w-[34rem] text-left text-[0.93rem]">
                <caption className="sr-only">What each key can do</caption>
                <thead>
                  <tr className="border-b border-mist/20 text-[0.82rem] text-mist/60">
                    <th scope="col" className="py-r3 pr-r3 font-medium">
                      Who can
                    </th>
                    <th scope="col" className="px-r3 py-r3 font-medium">
                      Agent key
                    </th>
                    <th scope="col" className="px-r3 py-r3 font-medium">
                      Izanagi key
                    </th>
                    <th scope="col" className="px-r3 py-r3 font-medium">
                      Both
                    </th>
                    <th scope="col" className="px-r3 py-r3 font-medium">
                      Neither
                    </th>
                  </tr>
                </thead>
                <tbody className="[&_.text-stone-light]:text-mist/35 [&_.text-sumi]:text-paper [&_.text-tide]:text-tide-light">
                  {trust.map((r) => (
                    <tr key={r.who} className="border-b border-mist/10 last:border-0">
                      <th scope="row" className="py-r3 pr-r3 font-medium text-paper">
                        {r.who}
                      </th>
                      {r.cells.map((c, i) => (
                        <td key={i} className="px-r3 py-r3">
                          <Mark c={c} />
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        </section>

        {/* connect */}
        <section id="connect" className="mx-auto grid max-w-[76rem] scroll-mt-r5 gap-r6 px-r4 py-r7 md:px-r5 lg:grid-cols-12">
          <div className="lg:col-span-5">
            <h2 className="display text-[2.2rem] md:text-[2.8rem]">Let your own agent pay through it</h2>
            <p className="mt-r4 max-w-[32rem] text-sumi-soft">
              Izanagi is a remote MCP server. Add it to Claude or any MCP client, and your agent can pay real x402 sellers in USDC on Base,
              with every payment screened, capped and stoppable. Watch it happen on the dashboard.
            </p>
            <Link to="/dashboard" className="btn btn-quiet mt-r4">
              Watch the dashboard
            </Link>
          </div>
          <div className="flex flex-col gap-r4 lg:col-span-7">
            <CopyField label="MCP server URL" value={mcpUrl} />
            <CopyField label="Claude Desktop config" value={claudeConfig} multiline />
          </div>
        </section>
      </main>
      <Footer />
    </div>
  );
}
