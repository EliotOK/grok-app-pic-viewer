// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mirrorCloseTransport, mirrorEnsureTransport, mirrorWsConnected } from "./mirrorTransport";

class Socket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSED = 3;
  static instances: Socket[] = [];
  readyState = Socket.CONNECTING;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: (() => void) | null = null;
  constructor(public url: string) { Socket.instances.push(this); }
  open() { this.readyState = Socket.OPEN; this.onopen?.(); }
  close() { this.readyState = Socket.CLOSED; this.onclose?.(); }
}

beforeEach(() => {
  vi.useFakeTimers();
  Socket.instances = [];
  vi.stubGlobal("WebSocket", Socket);
  window.__MIRROR__ = { token: "pairing-token" };
});

afterEach(() => {
  mirrorCloseTransport();
  delete window.__MIRROR__;
  delete window.__GROK_MIRROR_HEALTH__;
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("mirror transport recovery", () => {
  it("rejects a failed initial connection so bootstrap does not wait forever", async () => {
    const pending = mirrorEnsureTransport();
    const rejected = expect(pending).rejects.toThrow("closed before connection");
    expect(window.__GROK_MIRROR_HEALTH__?.connection).toBe("connecting");
    Socket.instances[0].close();
    await rejected;
    expect(window.__GROK_MIRROR_HEALTH__?.connection).toBe("closed");
    const retry = mirrorEnsureTransport();
    Socket.instances[1].open();
    await retry;
    expect(mirrorWsConnected()).toBe(true);
    expect(window.__GROK_MIRROR_HEALTH__?.connection).toBe("open");
  });

  it("keeps the current connection when an old socket closes late", async () => {
    const first = mirrorEnsureTransport();
    const old = Socket.instances[0];
    old.open();
    await first;
    mirrorCloseTransport();
    const next = mirrorEnsureTransport();
    Socket.instances[1].open();
    await next;
    old.onclose?.();
    expect(mirrorWsConnected()).toBe(true);
    expect(window.__GROK_MIRROR_HEALTH__?.connection).toBe("open");
  });
});
