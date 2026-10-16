import { useId, useState } from "react";
import { createT, type Locale } from "@/i18n";

export function ProviderErrorDetails({ detail, locale }: { detail: string; locale: Locale }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const tr = createT(locale);
  return (
    <div className="lobe-chat-error__details">
      <button
        type="button"
        className="error-banner__details-btn"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((value) => !value)}
      >
        {tr(open ? "error.hideDetails" : "error.details")}
      </button>
      {open ? <pre id={id} className="error-banner__detail">{detail}</pre> : null}
    </div>
  );
}
