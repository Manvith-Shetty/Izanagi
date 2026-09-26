import { useState } from "react";

export function CopyField({ label, value, multiline = false }: { label: string; value: string; multiline?: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard blocked: the text is still selectable */
    }
  };
  return (
    <div className="surface-raised rounded-2xl">
      <div className="flex items-center gap-r3 border-b border-rule/70 py-r2 pl-r4 pr-r2">
        <span className="text-[0.85rem] text-stone">{label}</span>
        <button type="button" onClick={copy} className="btn btn-ghost ml-auto min-h-9 text-[0.85rem]" aria-live="polite">
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      {multiline ? (
        <pre className="hex overflow-x-auto px-r4 py-r3 text-[0.85rem] leading-relaxed text-sumi-soft">{value}</pre>
      ) : (
        <p className="hex break-all px-r4 py-r3 text-[0.92rem] text-sumi">{value}</p>
      )}
    </div>
  );
}
