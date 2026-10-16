import { describe, expect, it } from "vitest";
import type { MessageSegment, MessageToolSegment } from "./types";
import { upsertToolInSegments, upsertToolsInSegments } from "./toolSegmentUpdates";

const tool = (id: string, extra: Partial<MessageToolSegment> = {}): MessageToolSegment => ({
  kind: "tool", toolCallId: id, title: "read file", status: "in_progress", ...extra,
});

describe("ordered tool segment updates", () => {
  it("shares unchanged segments and preserves richer fields on sparse updates", () => {
    const content: MessageSegment = { kind: "content", text: "reply" };
    const original = tool("call", { input: "argument", output: "stdout", detail: "full detail" });
    const base = [content, original];
    const next = upsertToolInSegments(base, tool("call", { status: "completed", title: "tool" }));
    expect(next[0]).toBe(content);
    expect(next[1]).toMatchObject({
      toolCallId: "call", title: "read file", status: "completed",
      input: "argument", output: "stdout", detail: "full detail",
    });
    expect(base).toEqual([content, original]);
    expect(original.status).toBe("in_progress");
  });

  it("matches sequential updates across duplicate IDs, host aliases and family changes", () => {
    const base: MessageSegment[] = [
      { kind: "thought", text: "thinking" },
      tool("duplicate"), tool("duplicate", { detail: "second duplicate" }),
      tool("first", { toolKind: "vision" }), tool("second", { toolKind: "vision" }),
      tool("", { title: "empty ID" }),
    ];
    const updates = [
      tool("first", { title: "normal", toolKind: "bash" }),
      tool("host-vision-alias", { status: "completed" }),
      tool("duplicate", { output: "result" }),
      tool("", { status: "completed" }),
      tool("host-x-new", { title: "search X", toolKind: "search" }),
      ...Array.from({ length: 200 }, (_, i) => tool(`call-${i % 19}`, {
        status: i % 2 ? "completed" : "in_progress", detail: `detail ${i}`,
      })),
    ];
    const expected = updates.reduce(upsertToolInSegments, base);
    expect(upsertToolsInSegments(base, updates)).toEqual(expected);
  });

  it("merges a large journal batch without copying every segment per update", () => {
    let titleReads = 0;
    const base: MessageSegment[] = Array.from({ length: 2_000 }, (_, i) => ({
      ...tool(`call-${i}`),
      get title() { titleReads++; return "read file"; },
    }));
    const updates = Array.from({ length: 2_000 }, (_, i) => tool(`call-${i}`, {
      status: "completed", output: `result ${i}`,
    }));
    const next = upsertToolsInSegments(base, updates);
    expect(next).toHaveLength(2_000);
    expect(next[1_999]).toMatchObject({ status: "completed", output: "result 1999" });
    expect(titleReads).toBeLessThan(10_000);
    expect((base[1_999] as MessageToolSegment).status).toBe("in_progress");
  });
});
