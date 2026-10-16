import { stripAnsi } from "./ansi";
import { redact } from "./redact";

/** Status carried by a provider response or structured CLI RPC error. */
export function providerHttpStatus(message: string | null | undefined): number | null {
  const raw = message ?? "";
  const structured = /\b(?:http_status|status_code)["']?\s*[:=]\s*([45]\d{2})\b/i;
  const text = /\b(?:http(?:\/\d(?:\.\d)?)?|status(?:\s+code)?)\s*[:=]?\s*([45]\d{2})\b/i;
  const match = raw.match(structured) ?? raw.match(text);
  return match ? Number(match[1]) : null;
}

export function providerErrorCode(message: string | null | undefined) {
  const status = providerHttpStatus(message);
  if (status === 403) return "REQUEST_REJECTED";
  if (status !== null && status >= 500) return "SERVICE_UNAVAILABLE";
  return null;
}

/** Bounded diagnostic text for provider failures, with credentials removed. */
export function providerErrorDetail(message: string | null | undefined, code?: string | null): string | null {
  if (!message?.trim()) return null;
  if (code !== "REQUEST_REJECTED" && code !== "SERVICE_UNAVAILABLE" && !providerErrorCode(message)) return null;
  const detail = redact(stripAnsi(message)).replace(
    /((?:["']?)(?:authorization|api[_-]?key|access[_-]?token|refresh[_-]?token)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;}]+)/gi,
    "$1[REDACTED]",
  );
  return detail.length > 4000 ? `${detail.slice(0, 4000)}…` : detail;
}
