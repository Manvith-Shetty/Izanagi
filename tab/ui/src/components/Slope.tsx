import { useEffect, useRef, useState } from "react";

// The hero instrument: an agent signs a voucher per call, the vouchers pile up unclaimed, and
// the seller cashes them in when the countdown runs out. Seal the pass before then and the
// escrow rejects the claim, even though every voucher was already signed.

const PRICE = 0.02;
const MAX = 12;
const SPAWN_MS = 650;
const CLAIM_IN = 10;

type Outcome = null | "claimed" | "rejected";

function restingPlace(i: number, outcome: Outcome) {
  const col = i % 6;
  const row = Math.floor(i / 6);
  if (outcome === "claimed") return { x: 91.5 + (i % 2) * 0.4, y: 56 - i * 2.2 };
  return { x: 35 + col * 7.6, y: 34 + row * 30 };
}

function Voucher({ i, outcome, sealed }: { i: number; outcome: Outcome; sealed: boolean }) {
  const [arrived, setArrived] = useState(false);
  useEffect(() => {
    const r = requestAnimationFrame(() => requestAnimationFrame(() => setArrived(true)));
    return () => cancelAnimationFrame(r);
  }, []);
  const { x, y } = arrived ? restingPlace(i, outcome) : { x: 3, y: 48 };
  const dead = sealed && outcome !== "claimed";
  const paid = outcome === "claimed";
  return (
    <div
      className="pointer-events-none absolute inset-0"
      style={{
        transform: `translate(${x}%, ${y}%)`,
        transition: `transform ${paid ? 900 : 1100}ms var(--ease-out-soft) ${paid ? i * 35 : 0}ms`,
      }}
    >
      <div
        className={`num flex h-7 w-12 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-md border text-[0.72rem] font-semibold transition-[rotate,scale,opacity] duration-500 ${
          dead
            ? "rotate-[-4deg] border-beni/60 bg-beni-wash text-beni line-through opacity-70"
            : paid
              ? "scale-90 border-tide/50 bg-tide-wash text-tide"
              : "border-kin/50 bg-paper text-kin"
        }`}
        style={{ boxShadow: dead ? "none" : "0 1px 0 rgb(22 33 31 / .05), 0 6px 12px -8px rgb(168 122 36 / .5)" }}
      >
        {PRICE.toFixed(2)}
      </div>
    </div>
  );
}

export function Slope() {
  const [run, setRun] = useState(0);
  const [count, setCount] = useState(0);
  const [left, setLeft] = useState(CLAIM_IN);
  const [sealed, setSealed] = useState(false);
  const [outcome, setOutcome] = useState<Outcome>(null);
  const sealedRef = useRef(false);

  useEffect(() => {
    setCount(0);
    setLeft(CLAIM_IN);
    setSealed(false);
    sealedRef.current = false;
    setOutcome(null);
    const spawn = setInterval(() => {
      if (sealedRef.current) return;
      setCount((c) => (c < MAX ? c + 1 : c));
    }, SPAWN_MS);
    const tick = setInterval(() => {
      setLeft((l) => {
        if (l <= 1) {
          clearInterval(tick);
          clearInterval(spawn);
          setOutcome(sealedRef.current ? "rejected" : "claimed");
          return 0;
        }
        return l - 1;
      });
    }, 1000);
    return () => {
      clearInterval(spawn);
      clearInterval(tick);
    };
  }, [run]);

  const seal = () => {
    sealedRef.current = true;
    setSealed(true);
  };
  const signed = (count * PRICE).toFixed(2);
  const done = outcome !== null;

  let message: string;
  if (outcome === "rejected") message = `Claim rejected by the escrow. The ${signed} USDC your agent already signed for stays in your wallet.`;
  else if (outcome === "claimed")
    message = `The seller cashed in ${signed} USDC. From here nobody can stop it, which is why Izanagi checks before the claim, not after.`;
  else if (sealed) message = "Sealed. When the seller claims, the escrow asks Izanagi whether these vouchers are still good. The answer is now no.";
  else message = "Each call signs a new voucher. The seller cashes them all in when the countdown ends. Stop it before then.";

  return (
    <section
      aria-label="How a signed payment gets stopped"
      className="surface-float relative overflow-hidden rounded-[1.25rem]"
    >
      <div className="flex flex-col gap-r4 border-b border-rule/70 px-r4 py-r4 md:flex-row md:items-center md:px-r5">
        <p className="text-[0.95rem] leading-snug text-sumi-soft">
          An agent paying <span className="font-semibold text-sumi">market-data</span>, one voucher per call
        </p>
        <dl className="flex gap-r5 md:ml-auto">
          <div>
            <dt className="text-[0.78rem] text-stone">Signed</dt>
            <dd className={`num text-[1.05rem] font-semibold ${sealed && outcome !== "claimed" ? "text-beni line-through" : "text-kin"}`}>
              {signed} USDC
            </dd>
          </div>
          <div>
            <dt className="text-[0.78rem] text-stone">Seller claims in</dt>
            <dd className="num text-[1.05rem] font-semibold">{done ? "done" : `0:${String(left).padStart(2, "0")}`}</dd>
          </div>
        </dl>
        {done ? (
          <button type="button" className="btn btn-quiet" onClick={() => setRun((r) => r + 1)}>
            {outcome === "claimed" ? "Try again and seal it in time" : "Replay"}
          </button>
        ) : (
          <button type="button" className="btn btn-seal" onClick={seal} disabled={sealed || count === 0}>
            {sealed ? "Sealed" : "Revoke this seller"}
          </button>
        )}
      </div>

      {/* the track */}
      <div className="relative h-[12.5rem] select-none md:h-[14rem]" aria-hidden="true">
        {/* held zone */}
        <div className="absolute inset-y-r4 left-[30%] right-[18%] rounded-xl bg-kin-wash/50" />
        {/* paid zone */}
        <div
          className={`absolute inset-y-r4 left-[85%] right-r3 rounded-xl bg-tide-wash transition-opacity duration-500 ${
            outcome === "claimed" ? "opacity-100" : "opacity-40"
          }`}
        />
        {/* the path the vouchers take */}
        <div className="absolute left-[3%] right-[18%] top-1/2 h-px bg-[repeating-linear-gradient(90deg,var(--color-stone-light)_0_6px,transparent_6px_12px)]" />
        {/* agent */}
        <div className="absolute left-[3%] top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-sumi" />
        {/* the gate */}
        <div
          className={`absolute inset-y-r3 left-[83%] w-[2px] -translate-x-1/2 rounded-full ${
            sealed ? "bg-beni" : "bg-sumi/70"
          }`}
        />
        {/* the stone */}
        <div
          className="absolute left-[83%] top-1/2 size-14 rounded-full md:size-16"
          style={{
            transform: sealed ? "translate(-50%, -50%) scale(1)" : "translate(-50%, -260%) scale(.7)",
            opacity: sealed ? 1 : 0,
            transition: "transform 620ms var(--ease-spring), opacity 200ms ease",
            background: "radial-gradient(circle at 34% 30%, #6c7b78 0%, #2a3835 55%, #16211f 100%)",
            boxShadow: "var(--shadow-seal), inset 0 -6px 12px rgb(0 0 0 / .35), inset 0 0 0 3px var(--color-beni)",
          }}
        />
        {Array.from({ length: count }, (_, i) => (
          <Voucher key={`${run}-${i}`} i={i} outcome={outcome} sealed={sealed} />
        ))}
      </div>

      <div className="grid grid-cols-[30%_53%_17%] border-t border-rule/70 text-[0.8rem] leading-snug text-stone md:text-[0.85rem]">
        <p className="px-r4 py-r3 md:px-r5">Agent signs</p>
        <p className="px-r2 py-r3">Signed, not yet claimed. Still stoppable.</p>
        <p className="py-r3 pr-r3">Claimed</p>
      </div>

      <p aria-live="polite" className={`min-h-[4.5rem] border-t border-rule/70 px-r4 py-r3 text-[0.95rem] md:px-r5 ${outcome === "rejected" ? "text-beni-deep" : "text-sumi-soft"}`}>
        {message}
      </p>
    </section>
  );
}
