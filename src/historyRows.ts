/**
 * DB row → `Message` conversion, shared by the History panel and the
 * background-turn re-attach path.
 *
 * The History panel lives in its own lazy chunk, so this converter sits in a
 * leaf module: `App.tsx` needs it when a webview reload re-attaches to a turn
 * that kept running in the background (the driver's seed transcript comes
 * from the same persisted rows), and importing the panel itself would drag
 * the whole lazy chunk into the boot graph.
 */
import type { HistoryRecord } from "./api";
import type {
  Attachment,
  ChatInjection,
  Message,
  ToolCallEntry,
} from "./types";

/**
 * Restore a multimodal `MessageContent` from its DB-stored string form.
 * Falls back to the raw string for plain text or malformed rows.
 */
export function parseStoredContent(raw: string): Message["content"] {
  if (!raw) return "";
  const trimmed = raw.trimStart();
  if (trimmed.startsWith("[")) {
    try {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) return parsed as Message["content"];
    } catch { /* fall through to plain text */ }
  }
  return raw;
}

/**
 * Restore the `Attachment[]` array from its DB-stored JSON form.
 * Returns `undefined` for missing/malformed rows so the message falls
 * back to the legacy `<details>` regex in `ChatMessage`.
 */
export function parseStoredAttachments(
  raw: string | undefined
): Attachment[] | undefined {
  if (!raw) return undefined;
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed) && parsed.length > 0) {
      return parsed as Attachment[];
    }
  } catch { /* fall through */ }
  return undefined;
}

/**
 * Convert the DB rows of one session into renderable chat messages. All
 * columns (tool calls, reasoning, attachments) are restored so the loaded
 * conversation renders exactly like the live one.
 */
export function recordsToMessages(sessionRecords: HistoryRecord[]): Message[] {
  const messages: Message[] = [];
  for (const r of sessionRecords) {
    let toolCalls: ToolCallEntry[] | undefined;
    if (r.tool_calls) {
      try {
        const parsed = (JSON.parse(r.tool_calls) as ToolCallEntry[]).map((e) => ({
          ...e,
          // A reload cannot resume a running step, but it must not invent a
          // success either: `running` becomes neutral ("skipped"), a real
          // failure stays an error, and the live-only thinking counter is
          // dropped (its text was never persisted).
          status:
            e.status === "error"
              ? ("error" as const)
              : e.status === "done" || e.status === "skipped"
                ? e.status
                : ("skipped" as const),
          thinking: undefined,
          // A pending inline confirmation is dead the moment the turn ended —
          // never replay its allow/deny buttons from persisted history.
          confirm: undefined,
        }));
        if (parsed.length > 0) toolCalls = parsed;
      } catch { /* ignore malformed */ }
    }
    const attachments = parseStoredAttachments(r.attachments);
    let injections: ChatInjection[] | undefined;
    if (r.injections) {
      try {
        const parsed = JSON.parse(r.injections) as ChatInjection[];
        if (Array.isArray(parsed) && parsed.length > 0) injections = parsed;
      } catch { /* ignore malformed */ }
    }
    let usageMeta: Message["usage_meta"];
    if (r.usage_meta) {
      try {
        const parsed = JSON.parse(r.usage_meta) as Message["usage_meta"];
        // Malformed JSON must never break a transcript; a partial object is
        // only usable when the duration and token counts exist.
        if (
          parsed &&
          typeof parsed.duration_ms === "number" &&
          typeof parsed.total_tokens === "number"
        ) {
          usageMeta = parsed;
        }
      } catch { /* ignore malformed */ }
    }
    messages.push({
      id: crypto.randomUUID(),
      role: r.role as "user" | "assistant",
      content: parseStoredContent(r.content),
      ...(attachments ? { attachments } : {}),
      ...(injections ? { injections } : {}),
      ...(usageMeta ? { usage_meta: usageMeta } : {}),
      tool_calls: toolCalls,
      ...(r.reasoning_content ? { reasoning_content: r.reasoning_content } : {}),
      ...(r.error ? { error: r.error } : {}),
      ...(r.status ? { status: r.status as Message["status"] } : {}),
      dbId: r.id,
    });
  }
  return messages;
}