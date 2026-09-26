import { useEffect, useState } from "react";
import QRCode from "qrcode";

/** A QR code for a phone to scan: the World App link that approves a request. */
export function Qr({ value, label }: { value: string; label: string }) {
  const [src, setSrc] = useState<string>();
  useEffect(() => {
    QRCode.toDataURL(value, { margin: 1, width: 360, color: { dark: "#16211f", light: "#f8faf9" } })
      .then(setSrc)
      .catch(() => setSrc(undefined));
  }, [value]);
  return src ? <img className="size-44 rounded-xl ring-1 ring-rule" src={src} alt={label} /> : <div className="size-44 rounded-xl bg-mist-deep" />;
}
