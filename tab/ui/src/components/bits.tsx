import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import QRCode from "qrcode";

export function Brand() {
  return (
    <Link to="/" className="brand" aria-label="Izanagi home">
      <span className="mark" aria-hidden="true" />
      Izanagi
    </Link>
  );
}

/** A QR code for a phone to scan, e.g. the World ID approval link. */
export function Qr({ value, label }: { value: string; label: string }) {
  const [src, setSrc] = useState<string>();
  useEffect(() => {
    QRCode.toDataURL(value, { margin: 1, width: 328, color: { dark: "#1e211f", light: "#ffffff" } })
      .then(setSrc)
      .catch(() => setSrc(undefined));
  }, [value]);
  return src ? <img className="qr" src={src} alt={label} /> : null;
}

/** Copy-to-clipboard with a field that shows exactly what gets copied. */
export function CopyField({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="copy">
      <code aria-label={label}>{value}</code>
      <button
        className="btn small ghost"
        onClick={async () => {
          await navigator.clipboard.writeText(value);
          setCopied(true);
          setTimeout(() => setCopied(false), 1600);
        }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

/** Seconds from now until `unix`, ticking. */
export function useCountdown(unix: number): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => clearInterval(t);
  }, []);
  return Math.max(0, Math.floor(unix - now));
}
