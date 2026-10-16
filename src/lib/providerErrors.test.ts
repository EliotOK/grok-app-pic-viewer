import { describe, expect, it } from "vitest";
import { providerErrorCode, providerErrorDetail, providerHttpStatus } from "./providerErrors";
import { buildErrorDeck, classifyErrorMessage, resolveErrorDeckCode } from "./errorDeck";
import { applyTurnError, formatTurnErrorBody, isAgentErrorCode, presentErrorBanner } from "./session/errors";

const rejected =
  'Internal error (code -32603, data: {"http_status":403,"message":"API error (status 403 Forbidden): permission-denied: I cannot help with this request."})';
const unavailable =
  "API error (status 503 Service Unavailable): Authentication is temporarily unavailable. Please retry.";

describe("provider response errors", () => {
  it.each([
    [rejected, 403],
    [unavailable, 503],
    ["HTTP/1.1 502 Bad Gateway", 502],
    ['{"status_code":504}', 504],
    ["status code: 500", 500],
    ["file 403.txt: permission denied", null],
    ["Agent process exited (code 503)", null],
    ["status 5030", null],
    ["HTTP 401 Unauthorized", 401],
  ])("extracts only an HTTP status from %s", (message, expected) => {
    expect(providerHttpStatus(message)).toBe(expected);
  });

  it("classifies a provider refusal and refines older crash errors", () => {
    expect(providerErrorCode(rejected)).toBe("REQUEST_REJECTED");
    expect(classifyErrorMessage(rejected)).toBe("REQUEST_REJECTED");
    expect(resolveErrorDeckCode("AGENT_CRASHED", rejected)).toBe("REQUEST_REJECTED");
    expect(isAgentErrorCode("REQUEST_REJECTED")).toBe(true);
    const card = buildErrorDeck("REQUEST_REJECTED", "zh");
    expect(card.problem).toBe("模型服务拒绝了请求");
    expect(card.cause).toContain("403");
    expect(card.primary.id).toBe("open_doctor");
    expect(card.secondary?.id).toBe("dismiss");
    expect(formatTurnErrorBody({ code: "AGENT_CRASHED", message: rejected }, "zh")).toContain(card.problem);
  });

  it("keeps temporary authentication service failures out of login recovery", () => {
    expect(classifyErrorMessage(unavailable)).toBe("SERVICE_UNAVAILABLE");
    expect(resolveErrorDeckCode("AUTH_FAILED", unavailable)).toBe("SERVICE_UNAVAILABLE");
    expect(isAgentErrorCode("SERVICE_UNAVAILABLE")).toBe(true);
    const card = buildErrorDeck("SERVICE_UNAVAILABLE", "en");
    expect(card.primary.id).toBe("dismiss");
    expect(card.secondary?.id).toBe("open_doctor");
    expect(formatTurnErrorBody({ code: "AUTH_FAILED", message: unavailable }, "en")).toContain(card.problem);
  });

  it("recognizes stored errors with both supported code prefixes", () => {
    expect(formatTurnErrorBody({ content: `**REQUEST_REJECTED**\n${rejected}` }, "zh")).toContain("模型服务拒绝了请求");
    expect(formatTurnErrorBody({ content: `SERVICE_UNAVAILABLE: ${unavailable}` }, "zh")).toContain("模型服务暂时不可用");
  });

  it("retains distinct authentication, tool permission and process exit errors", () => {
    expect(resolveErrorDeckCode("AUTH_FAILED", "HTTP 401 Unauthorized")).toBe("AUTH_FAILED");
    expect(classifyErrorMessage("read_file: permission denied: 403.txt")).toBe("PERMISSION_DENIED");
    expect(resolveErrorDeckCode("AGENT_CRASHED", "Agent process exited (code 503)")).toBe("AGENT_CRASHED");
    expect(resolveErrorDeckCode("AGENT_CRASHED", "HTTP 403; bwrap: setting up uid map: Permission denied")).toBe("SANDBOX_BLOCKED");
  });

  it("keeps a redacted provider reason on both the error row and banner", () => {
    const message = `${rejected}; api_key="opaque-secret"; Authorization: Bearer secretvalue12345`;
    const [row] = applyTurnError([], { code: "REQUEST_REJECTED", message }, "zh");
    const banner = presentErrorBanner({ code: "REQUEST_REJECTED", message }, null, "zh");
    expect(row?.content).toContain("模型服务拒绝了请求");
    expect(row?.errorDetail).toContain("permission-denied");
    expect(row?.errorDetail).not.toMatch(/opaque-secret|secretvalue12345/);
    expect(banner?.detail).toBe(row?.errorDetail);
    expect(banner?.reconnectHint).toBe(false);
    expect(presentErrorBanner({ code: "SERVICE_UNAVAILABLE", message: "HTTP 504 Gateway Timeout" }, null, "zh")?.summary).toBe("模型服务暂时不可用");
    expect(presentErrorBanner(null, `REQUEST_REJECTED: ${message}`, "zh")?.detail).toContain("permission-denied");
    expect(providerErrorDetail(`HTTP 503 ${"x".repeat(5000)}`)?.length).toBe(4001);
  });
});
