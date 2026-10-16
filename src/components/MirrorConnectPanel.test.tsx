// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { MirrorConnectPanel, type MirrorConnectLabels } from "./MirrorConnectPanel";
import * as api from "@/lib/api";

vi.mock("@/lib/api", () => ({
  mirrorStatus: vi.fn(),
  mirrorStart: vi.fn(),
}));

afterEach(() => { cleanup(); vi.clearAllMocks(); });

it("keeps a stopped host stopped until Start is selected", async () => {
  const stopped: api.MirrorStatus = {
    running: false, publicUrl: null, localPort: null, token: null, tokenTail: null,
    clients: 0, maxClients: 4, phase: "stopped", error: null, readOnly: true,
    allowLan: false, lanUrl: null,
  };
  vi.mocked(api.mirrorStatus).mockResolvedValue(stopped);
  vi.mocked(api.mirrorStart).mockResolvedValue({ ...stopped, running: true, phase: "local" });
  const labels = new Proxy({} as MirrorConnectLabels, { get: (_, key) => String(key) });
  render(<MirrorConnectPanel variant="inline" labels={labels} locale="en"
    onRequestConfirm={vi.fn()} showToast={vi.fn()} />);
  await waitFor(() => expect(api.mirrorStatus).toHaveBeenCalled());
  expect(api.mirrorStart).not.toHaveBeenCalled();
  await waitFor(() => expect((screen.getByRole("button", { name: "start" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "start" }));
  await waitFor(() => expect(api.mirrorStart).toHaveBeenCalledTimes(1));
});
