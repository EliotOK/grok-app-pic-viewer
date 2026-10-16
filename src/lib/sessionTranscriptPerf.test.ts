import { describe, expect, it } from "vitest";
import { filterTranscriptMessages, type ChatMessage } from "./session";

describe("tool-heavy transcript filtering", () => {
  it("indexes assistant tools once while preserving standalone results", () => {
    let segmentReads = 0;
    const segments = Array.from({ length: 2_000 }, (_, i) => ({
      kind: "tool" as const,
      toolCallId: `call-${i}`,
      title: "read file",
      status: "completed",
    }));
    const assistant: ChatMessage = {
      id: "answer", role: "assistant", content: "Working",
      get segments() { segmentReads++; return segments; },
    };
    const tools: ChatMessage[] = segments.map((segment) => ({
      id: `tool-${segment.toolCallId}`, role: "tool", content: "result",
      marker: "tool_step", toolCallId: segment.toolCallId,
    }));
    const orphan: ChatMessage = {
      id: "tool-orphan", role: "tool", marker: "tool_step", content: "orphan result",
    };
    expect(filterTranscriptMessages([assistant, ...tools, orphan]))
      .toEqual([assistant, orphan]);
    expect(segmentReads).toBeLessThanOrEqual(4);
  });
});
