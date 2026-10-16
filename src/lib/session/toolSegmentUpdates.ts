import type { MessageSegment, MessageToolSegment } from "./types";
import { hostToolFamilyKey, isGenericToolLabel } from "./segments";

/** Preserve stable IDs and richer arguments/output on sparse status ticks. */
function mergeTool(prev: MessageToolSegment, tool: MessageToolSegment): MessageToolSegment {
  const title =
    (tool.title && !isGenericToolLabel(tool.title) ? tool.title : "") || prev.title;
  const detail = (tool.detail || "").length >= (prev.detail || "").length
    ? tool.detail || prev.detail : prev.detail || tool.detail;
  return {
    ...prev,
    ...tool,
    toolCallId: prev.toolCallId || tool.toolCallId,
    title,
    detail,
    path: tool.path || prev.path,
    input: tool.input || prev.input,
    output: (tool.output || "").trim() ? tool.output : prev.output,
    meta: tool.meta || prev.meta,
    toolKind: tool.toolKind || prev.toolKind,
  };
}

/** Copy the timeline once and replace only the updated tool. */
export function upsertToolInSegments(
  segs: MessageSegment[], tool: MessageToolSegment,
): MessageSegment[] {
  const next = segs.slice();
  let index = next.findIndex((s) => s.kind === "tool" && s.toolCallId === tool.toolCallId);
  if (index < 0) {
    const family = hostToolFamilyKey(tool.toolCallId, tool.toolKind, tool.title);
    if (family) index = next.findIndex((s) => s.kind === "tool" &&
      hostToolFamilyKey(s.toolCallId, s.toolKind, s.title) === family);
  }
  if (index < 0) next.push({ ...tool });
  else next[index] = mergeTool(next[index] as MessageToolSegment, tool);
  return next;
}

/** Apply an ordered journal batch with one timeline copy and indexed lookups. */
export function upsertToolsInSegments(
  segs: MessageSegment[], tools: MessageToolSegment[],
): MessageSegment[] {
  const next = segs.slice();
  const ids = new Map<string, number>();
  const families = new Map<string, number>();
  const first = (map: Map<string, number>, key: string | null, index: number) => {
    if (key != null && index < (map.get(key) ?? Infinity)) map.set(key, index);
  };
  const indexTool = (tool: MessageToolSegment, index: number) => {
    first(ids, tool.toolCallId, index);
    first(families, hostToolFamilyKey(tool.toolCallId, tool.toolKind, tool.title), index);
  };
  next.forEach((s, index) => { if (s.kind === "tool") indexTool(s, index); });

  // Family labels can change during an update; repair only an invalidated entry.
  const lookup = (
    map: Map<string, number>, key: string,
    matches: (s: MessageToolSegment) => boolean,
  ): number | undefined => {
    const index = map.get(key);
    if (index == null) return undefined;
    const current = next[index];
    if (current?.kind === "tool" && matches(current)) return index;
    const replacement = next.findIndex((s) => s.kind === "tool" && matches(s));
    if (replacement < 0) { map.delete(key); return undefined; }
    map.set(key, replacement);
    return replacement;
  };
  for (const tool of tools) {
    let index = lookup(ids, tool.toolCallId, (s) => s.toolCallId === tool.toolCallId);
    if (index == null) {
      const family = hostToolFamilyKey(tool.toolCallId, tool.toolKind, tool.title);
      if (family) index = lookup(families, family, (s) =>
        hostToolFamilyKey(s.toolCallId, s.toolKind, s.title) === family);
    }
    if (index == null) {
      index = next.length;
      next.push({ ...tool });
    } else {
      next[index] = mergeTool(next[index] as MessageToolSegment, tool);
    }
    indexTool(next[index] as MessageToolSegment, index);
  }
  return next;
}
