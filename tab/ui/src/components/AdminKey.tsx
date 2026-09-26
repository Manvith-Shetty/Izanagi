import { useEffect, useRef, useState } from "react";
import { adminToken, setAdminToken } from "../lib/api";

// The dashboard's buttons act through Tab with its admin token (TAB_ADMIN_TOKEN). It's kept for
// this browser tab only, never in localStorage.
export function AdminKey({ open, onClose, onSaved }: { open: boolean; onClose: () => void; onSaved: () => void }) {
  const [value, setValue] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) {
      setValue(adminToken() ?? "");
      requestAnimationFrame(() => input.current?.focus());
    }
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center p-r4 sm:items-center" role="dialog" aria-modal="true" aria-labelledby="admin-title">
      <button type="button" aria-label="Close" className="absolute inset-0 cursor-default bg-sumi/30 animate-[fade_200ms_ease]" onClick={onClose} />
      <form
        className="surface-float relative w-full max-w-[26rem] animate-[rise_320ms_var(--ease-spring)] rounded-2xl p-r5"
        onSubmit={(e) => {
          e.preventDefault();
          setAdminToken(value.trim() || null);
          onSaved();
        }}
      >
        <h2 id="admin-title" className="display text-[1.5rem] tracking-[-0.02em]">
          Unlock the dashboard
        </h2>
        <p className="mt-r2 text-[0.93rem] text-sumi-soft">
          Closing and reopening tabs needs Tab's admin token, the <span className="hex">TAB_ADMIN_TOKEN</span> in <span className="hex">tab/.env</span>. It stays in this browser tab only.
        </p>
        <label className="mt-r4 block text-[0.85rem] text-stone" htmlFor="admin-token">
          Admin token
        </label>
        <input
          ref={input}
          id="admin-token"
          type="password"
          autoComplete="off"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          className="hex mt-r1 w-full rounded-lg border border-rule bg-paper px-r3 py-r2 text-[0.95rem] outline-none focus-visible:border-tide focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-tide"
        />
        <div className="mt-r4 flex justify-end gap-r2">
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={!value.trim()}>
            Unlock
          </button>
        </div>
      </form>
    </div>
  );
}
