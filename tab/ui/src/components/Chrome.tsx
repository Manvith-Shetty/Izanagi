import { NavLink, Link } from "react-router-dom";
import type { ReactNode } from "react";

export function Mark({ className = "size-8" }: { className?: string }) {
  // the boulder at the foot of the slope: what Izanagi rolled across the pass
  return (
    <svg viewBox="0 0 32 32" className={className} aria-hidden="true">
      <rect width="32" height="32" rx="8" fill="var(--color-sumi)" />
      <path d="M5 9 L19 23 L27 23" fill="none" stroke="var(--color-tide-light)" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx="20.5" cy="17" r="5.5" fill="var(--color-mist)" />
    </svg>
  );
}

const nav = [
  { to: "/#how", label: "How it works" },
  { to: "/sellers", label: "For sellers" },
  { to: "/dashboard", label: "Dashboard" },
];

export function Header({ children }: { children?: ReactNode }) {
  return (
    <header className="relative z-20">
      <div className="mx-auto flex max-w-[76rem] items-center gap-r4 px-r4 py-r4 md:px-r5">
        <Link
          to="/"
          className="group -m-r2 flex items-center gap-r3 rounded-lg p-r2 transition-opacity duration-150 hover:opacity-85 active:opacity-70"
        >
          <Mark className="size-8 transition-transform duration-300 ease-[var(--ease-spring)] group-hover:-rotate-6" />
          <span className="display text-[1.35rem] leading-none">Izanagi</span>
        </Link>
        <nav className="ml-auto hidden items-center gap-r1 md:flex" aria-label="Main">
          {nav.map((n) => (
            <NavLink
              key={n.to}
              to={n.to}
              className={({ isActive }) =>
                `btn btn-ghost min-h-9 text-[0.92rem] font-medium ${isActive && !n.to.includes("#") ? "text-tide" : ""}`
              }
            >
              {n.label}
            </NavLink>
          ))}
        </nav>
        <div className="ml-auto flex items-center gap-r2 md:ml-r2">{children}</div>
      </div>
      <nav className="mx-auto flex max-w-[76rem] gap-r1 overflow-x-auto px-r3 pb-r2 md:hidden" aria-label="Main">
        {nav.map((n) => (
          <NavLink key={n.to} to={n.to} className="btn btn-ghost min-h-9 shrink-0 text-[0.9rem] font-medium">
            {n.label}
          </NavLink>
        ))}
      </nav>
    </header>
  );
}

export function Footer() {
  return (
    <footer className="border-t border-rule/80">
      <div className="mx-auto flex max-w-[76rem] flex-col gap-r3 px-r4 py-r5 text-[0.9rem] text-stone md:flex-row md:items-center md:px-r5">
        <div className="flex items-center gap-r3">
          <Mark className="size-6" />
          <span>Izanagi. Payments your agent made, that you can still stop.</span>
        </div>
        <div className="flex gap-r4 md:ml-auto">
          <a className="link" href="https://github.com/x402-foundation/x402" target="_blank" rel="noreferrer">
            x402 batch settlement
          </a>
          <Link className="link" to="/sellers">
            For sellers
          </Link>
          <Link className="link" to="/dashboard">
            Dashboard
          </Link>
        </div>
      </div>
    </footer>
  );
}

export function SampleNotice({ show }: { show: boolean }) {
  if (!show) return null;
  return (
    <div role="status" className="relative z-20 border-b border-kin/25 bg-kin-wash/80 text-[0.88rem] text-sumi-soft">
      <p className="mx-auto max-w-[76rem] px-r4 py-r2 md:px-r5">
        The Tab server isn't answering at <span className="hex">/api</span>, so this page is showing sample data. Start it with{" "}
        <span className="hex">cargo run -p tab</span> to see your own wallet.
      </p>
    </div>
  );
}
