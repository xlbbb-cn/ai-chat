/**
 * Background turn driver: the frontend mirror of the backend `LlmTurn` base.
 *
 * When the user opens a new chat (or switches history) while a reply is still
 * streaming, the foreground turn machinery hands the running turn to a driver
 * here. The driver keeps listening to that turn's events, accumulates the
 * transcript, and persists the assistant row when the turn settles — exactly
 * like the foreground would — so a background conversation never loses its
 * reply. When the user switches back, the driver's live transcript is shown
 * and can be stopped or injected into; once it settles, the foreground takes
 * over the session again.
 *
 * Ownership rule: a session is owned by EITHER the foreground machinery OR a
 * driver, never both. The global App listeners skip any session that has a
 * driver (see `hasDriver`), and the driver only processes its own session's
 * events. Handoff subscribes the driver unarmed, then arms it and tears down
 * the foreground listeners in one synchronous step, so no event is lost or
 * double-processed across the boundary.
 */
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import {
  injectUserMessage,
  saveHistory,
  stopChatCompletion,
} from "./api";
import type { TurnSnapshot } from "./api";
import type {
  ChatInjection,
  Message,
  ToolCallEntry,
  TurnUsageMeta,
} from "./types";
import { stripControlMarkers, stripTrailingControlMarkers } from "./turnMarkers";

export interface TurnStats {
  started_at: number;
  prompt: number;
  completion: number;
  total: number;
  cached: number;
  cache_reported: boolean;
}

/** Everything the foreground captured at the handoff instant. */
export interface TurnSeed {
  sessionId: string;
  turnId: string;
  assistantId: string;
  messages: Message[];
  content: string;
  reasoning: string;
  toolCalls: ToolCallEntry[];
  injections: ChatInjection[];
  pendingInjections: ChatInjection[];
  stats: TurnStats | null;
}

export interface PendingConfirm {
  request_id: string;
  call_id?: string | null;
  reason: string;
  cmd_type: string;
  code: string;
  confirm_kind?: string;
  requires_auth?: string;
  risk_level?: string;
  risk_score?: number;
}

/** App-provided hooks; the driver stays free of React and global state. */
export interface DriverHost {
  translate: (key: string, vars?: Record<string, unknown>) => string;
  notifySettled: (ok: boolean, body: string) => void;
  notifyBlocked: (title: string, body: string, kind: string) => void;
  /** Undelivered injections become the session's next user turn. */
  onUndelivered: (sessionId: string, items: { text: string }[]) => void;
  /** A risky command awaiting the user (badge + dialog when visible). */
  onConfirm: (sessionId: string, payload: PendingConfirm | null) => void;
}

type TranscriptListener = (messages: Message[]) => void;

export class BackgroundTurnDriver {
  readonly sessionId: string;
  readonly turnId: string;

  private host: DriverHost;
  private armed = false;
  private settled = false;
  private stopping = false;

  private assistantId = "";
  private messages: Message[] = [];
  private content = "";
  private reasoning = "";
  private toolCalls: ToolCallEntry[] = [];
  private injections: ChatInjection[] = [];
  private pendingInjections: ChatInjection[] = [];
  private stats: TurnStats | null = null;
  private hasRunningTool = false;
  private pendingConfirm: PendingConfirm | null = null;

  private unlisteners: UnlistenFn[] = [];
  private transcriptListeners = new Set<TranscriptListener>();
  private settledListeners = new Set<(messages: Message[]) => void>();

  constructor(host: DriverHost, sessionId: string, turnId: string) {
    this.host = host;
    this.sessionId = sessionId;
    this.turnId = turnId;
  }

  /**
   * Re-attach to a turn that kept running through a webview reload: rebuild the
   * live assistant row from the backend snapshot plus the persisted messages,
   * subscribe unarmed, then arm in one step. The caller only wires up its own
   * transcript/settled callbacks — the snapshot → transcript mapping lives
   * here, next to the driver that consumes it.
   */
  static async attach(
    host: DriverHost,
    sessionId: string,
    snapshot: TurnSnapshot,
    persisted: Message[],
  ): Promise<BackgroundTurnDriver> {
    const assistantId = crypto.randomUUID();
    const toolCalls: ToolCallEntry[] = snapshot.tool_calls.map((tc) => ({
      task_id: tc.task_id,
      agent_name: host.translate("app.agents.tool"),
      ...(tc.kind ? { kind: tc.kind } : { kind: "tool" as const }),
      description: tc.description,
      status: tc.status,
      summary: tc.summary,
      started_at: snapshot.started_at_ms,
    }));
    const injections: ChatInjection[] = snapshot.injections.map((item) => ({
      id: item.id,
      text: item.text,
      created_at_ms: item.created_at_ms,
      ...(item.kind === "background_result"
        ? { kind: "background_result" as const }
        : {}),
    }));
    const assistantRow: Message = {
      id: assistantId,
      role: "assistant",
      content: snapshot.content,
      streaming: true,
      ...(snapshot.reasoning ? { reasoning_content: snapshot.reasoning } : {}),
      ...(toolCalls.length ? { tool_calls: toolCalls } : {}),
      ...(injections.length ? { injections } : {}),
    };
    const driver = new BackgroundTurnDriver(host, sessionId, snapshot.turn_id);
    await driver.subscribe();
    driver.arm({
      sessionId,
      turnId: snapshot.turn_id,
      assistantId,
      messages: [...persisted, assistantRow],
      content: snapshot.content,
      reasoning: snapshot.reasoning,
      toolCalls,
      injections,
      pendingInjections: [],
      stats: {
        started_at: snapshot.started_at_ms,
        prompt: snapshot.usage.prompt_tokens,
        completion: snapshot.usage.completion_tokens,
        total: snapshot.usage.prompt_tokens + snapshot.usage.completion_tokens,
        cached: snapshot.usage.cached_tokens,
        cache_reported: snapshot.usage.cache_reported,
      },
    });
    return driver;
  }

  // ── Lifecycle ─────────────────────────────────────────────────────────────

  /** Register the event listeners (unarmed: they ignore everything until arm). */
  async subscribe(): Promise<void> {
    const mine = (payload: { turn_id?: string } | null | undefined) =>
      payload?.turn_id === this.turnId;
    const forSession = (payload: { session_id?: string | null }) =>
      !payload.session_id || payload.session_id === this.sessionId;
    const guard = () => this.armed && !this.settled;

    const un = await Promise.all([
      listen<{ turn_id?: string; text: string }>("chat-token", (e) => {
        if (!guard() || !mine(e.payload)) return;
        this.onToken(e.payload.text);
      }),
      listen<{ turn_id?: string; text: string }>("chat-reasoning-token", (e) => {
        if (!guard() || !mine(e.payload)) return;
        this.reasoning += e.payload.text;
        this.patchAssistant({ reasoning_content: this.reasoning });
      }),
      listen<{ turn_id?: string; content: string; reasoning: string }>("chat-retry", (e) => {
        if (!guard() || !mine(e.payload)) return;
        this.onRetry(e.payload.content, e.payload.reasoning);
      }),
      listen<{ turn_id?: string }>("chat-done", (e) => {
        if (!mine(e.payload)) return;
        if (!this.armed || this.settled) return;
        this.settle(this.stopping ? "stopped" : "done");
      }),
      listen<{ turn_id?: string; error: string }>("chat-error", (e) => {
        if (!mine(e.payload)) return;
        if (!this.armed || this.settled) return;
        this.settle("error", e.payload.error || "the turn failed");
      }),
      listen<{
        prompt_tokens: number;
        completion_tokens: number;
        total_tokens?: number;
        cached_tokens?: number;
        session_id?: string | null;
      }>("chat-usage", (e) => {
        if (!guard() || !forSession(e.payload)) return;
        if (!this.stats) return;
        this.stats.prompt += e.payload.prompt_tokens;
        this.stats.completion += e.payload.completion_tokens;
        this.stats.total +=
          e.payload.total_tokens ?? e.payload.prompt_tokens + e.payload.completion_tokens;
        if (e.payload.cached_tokens !== undefined) {
          this.stats.cached += e.payload.cached_tokens;
          this.stats.cache_reported = true;
        }
      }),
      listen<{
        session_id?: string | null;
        call_id?: string | null;
        phase?: "progress" | "done";
        ok?: boolean;
        text: string;
      }>("tool-call", (e) => {
        if (!guard() || !forSession(e.payload)) return;
        this.onToolCall(e.payload);
      }),
      listen<{ session_id?: string | null; skill_name: string; call_id?: string | null }>(
        "skill-loaded",
        (e) => {
          if (!guard() || !forSession(e.payload)) return;
          this.onSkillLoaded(e.payload.skill_name, e.payload.call_id ?? undefined);
        }
      ),
      listen<{ items: ChatInjection[]; session_id?: string }>("chat-injected", (e) => {
        if (!guard() || !forSession(e.payload)) return;
        this.onInjected(e.payload.items ?? []);
      }),
      // Sub-agent mission rows: terminal states + activity. Token streams are
      // appended to the row summary (capped) so a background mission still
      // shows progress without the foreground's 400 ms buffer machinery.
      listen<{ session_id?: string | null; task_id: string; agent_id?: string; agent_name?: string; description?: string }>(
        "agent-task-start",
        (e) => {
          if (!guard() || !forSession(e.payload)) return;
          this.upsertAgentRow(e.payload.task_id, {
            agent_name: e.payload.agent_name || e.payload.agent_id || "agent",
            description: e.payload.description || e.payload.task_id,
            status: "running",
          });
        }
      ),
      listen<{ session_id?: string | null; task_id: string; status?: string }>(
        "agent-task-done",
        (e) => {
          if (!guard() || !forSession(e.payload)) return;
          this.upsertAgentRow(e.payload.task_id, {
            status: e.payload.status === "stopped" ? "skipped" : "done",
          });
        }
      ),
      listen<{ session_id?: string | null; task_id: string; error?: string }>(
        "agent-task-error",
        (e) => {
          if (!guard() || !forSession(e.payload)) return;
          this.upsertAgentRow(e.payload.task_id, { status: "error", error: e.payload.error });
        }
      ),
      listen<{ session_id?: string | null; task_id: string }>(
        "agent-task-skipped",
        (e) => {
          if (!guard() || !forSession(e.payload)) return;
          this.upsertAgentRow(e.payload.task_id, { status: "skipped" });
        }
      ),
      listen<{ session_id?: string | null; task_id: string; phase?: string; tool?: string; detail?: string; text?: string }>(
        "agent-task-tool",
        (e) => {
          if (!guard() || !forSession(e.payload)) return;
          const label = [e.payload.tool, e.payload.detail].filter(Boolean).join(" · ").trim();
          const activity = (e.payload.text || label).trim().slice(0, 200);
          if (activity) this.upsertAgentRow(e.payload.task_id, { activity });
        }
      ),
      listen<{ session_id?: string | null; task_id: string; token?: string }>(
        "agent-task-token",
        (e) => {
          if (!guard() || !forSession(e.payload)) return;
          if (!e.payload.token) return;
          this.appendAgentSummary(e.payload.task_id, e.payload.token);
        }
      ),
      listen<PendingConfirm & { session_id?: string | null }>("confirm-required", (e) => {
        if (!this.armed || this.settled) return;
        if (e.payload.session_id && e.payload.session_id !== this.sessionId) return;
        this.pendingConfirm = e.payload;
        this.host.onConfirm(this.sessionId, e.payload);
        this.host.notifyBlocked(
          this.host.translate("app.notify.confirmTitle"),
          e.payload.reason || e.payload.code.split("\n")[0] || "",
          "confirm"
        );
      }),
    ]);
    this.unlisteners = un;
  }

  /** Seed the captured state and start processing. Synchronous: the caller
   *  tears down the foreground listeners in the same task. */
  arm(seed: TurnSeed): void {
    this.assistantId = seed.assistantId;
    this.messages = seed.messages;
    this.content = seed.content;
    this.reasoning = seed.reasoning;
    this.toolCalls = seed.toolCalls;
    this.injections = seed.injections;
    this.pendingInjections = seed.pendingInjections;
    this.stats = seed.stats;
    this.hasRunningTool = this.toolCalls.some(
      (tc) => tc.status === "running" && tc.kind !== "agent" && tc.kind !== "planner"
    );
    this.armed = true;
    this.emit();
  }

  /** Stop this turn (the backend cancels; the terminal event settles it). */
  async stop(): Promise<void> {
    this.stopping = true;
    try {
      await stopChatCompletion(this.sessionId);
    } catch {
      // The turn may have already settled; the terminal event still arrives.
    }
  }

  /** Push a user message into the running turn (the composer's Enter path). */
  async inject(text: string): Promise<void> {
    const trimmed = text.trim();
    if (!trimmed || this.settled) return;
    const item: ChatInjection = {
      id: crypto.randomUUID(),
      text: trimmed,
      created_at_ms: Date.now(),
    };
    this.pendingInjections = [...this.pendingInjections, item];
    this.emit();
    try {
      const res = await injectUserMessage(this.sessionId, item.id, trimmed);
      if (!res.accepted) {
        this.pendingInjections = this.pendingInjections.filter((i) => i.id !== item.id);
        this.host.onUndelivered(this.sessionId, [{ text: trimmed }]);
        this.emit();
      }
    } catch {
      this.pendingInjections = this.pendingInjections.filter((i) => i.id !== item.id);
      this.host.onUndelivered(this.sessionId, [{ text: trimmed }]);
      this.emit();
    }
  }

  dispose(): void {
    this.settled = true;
    this.unlisteners.forEach((fn) => fn());
    this.unlisteners = [];
    this.transcriptListeners.clear();
  }

  // ── Views ─────────────────────────────────────────────────────────────────

  getMessages(): Message[] {
    return this.messages;
  }

  getPendingInjections(): ChatInjection[] {
    return this.pendingInjections;
  }

  getPendingConfirm(): PendingConfirm | null {
    return this.pendingConfirm;
  }

  isSettled(): boolean {
    return this.settled;
  }

  onTranscript(cb: TranscriptListener): () => void {
    this.transcriptListeners.add(cb);
    return () => this.transcriptListeners.delete(cb);
  }

  onSettled(cb: (messages: Message[]) => void): () => void {
    this.settledListeners.add(cb);
    return () => this.settledListeners.delete(cb);
  }

  private emit(): void {
    for (const cb of this.transcriptListeners) cb(this.messages);
  }

  // ── Transcript mutations ──────────────────────────────────────────────────

  private patchAssistant(patch: Partial<Message>): void {
    this.messages = this.messages.map((m) =>
      m.id === this.assistantId ? { ...m, ...patch } : m
    );
    this.emit();
  }

  private setToolCalls(next: ToolCallEntry[]): void {
    this.toolCalls = next;
    this.hasRunningTool = next.some(
      (tc) => tc.status === "running" && tc.kind !== "agent" && tc.kind !== "planner"
    );
    this.patchAssistant({ tool_calls: next });
  }

  private onToken(token: string): void {
    this.content = stripTrailingControlMarkers(this.content + token);
    let toolCalls = this.toolCalls;
    if (this.hasRunningTool) {
      toolCalls = toolCalls.map((e) =>
        e.status === "running" && e.kind !== "agent" && e.kind !== "planner"
          ? { ...e, status: "done" as const }
          : e
      );
      this.hasRunningTool = false;
      this.toolCalls = toolCalls;
    }
    this.messages = this.messages.map((m) =>
      m.id === this.assistantId
        ? { ...m, content: this.content, tool_calls: toolCalls }
        : m
    );
    this.emit();
  }

  private onRetry(content: string, reasoning: string): void {
    const tail = stripControlMarkers(content);
    if (tail && this.content.endsWith(tail)) {
      this.content = this.content.slice(0, this.content.length - tail.length);
    }
    if (reasoning && this.reasoning.endsWith(reasoning)) {
      this.reasoning = this.reasoning.slice(0, this.reasoning.length - reasoning.length);
    }
    this.patchAssistant({ content: this.content, reasoning_content: this.reasoning });
  }

  private onToolCall(payload: {
    call_id?: string | null;
    phase?: "progress" | "done";
    ok?: boolean;
    text: string;
  }): void {
    const text = payload.text ?? "";
    const firstLine = text.split("\n")[0];
    const labelMatch = firstLine.match(/^(.*?)\*(.+?)\*$/);
    let label: string;
    if (labelMatch) {
      const emoji = labelMatch[1].trim();
      label = emoji ? `${emoji} ${labelMatch[2]}` : labelMatch[2];
    } else {
      label = firstLine.replace(/[*]/g, "").trim();
    }
    const codeMatch = text.match(/```(?:\w+)?\n([\s\S]+?)\n```/);
    const detail = codeMatch ? codeMatch[1].trim() : undefined;
    const toolName = this.host.translate("app.agents.tool");
    const callId = payload.call_id ?? undefined;

    if (callId) {
      const existing = this.toolCalls.some((tc) => tc.task_id === callId);
      if (payload.phase === "done") {
        const status = payload.ok === false ? ("error" as const) : ("done" as const);
        this.setToolCalls(
          existing
            ? this.toolCalls.map((tc) =>
                tc.task_id === callId ? { ...tc, status, summary: tc.summary ?? detail } : tc
              )
            : [
                ...this.toolCalls,
                { task_id: callId, agent_name: toolName, kind: "tool", description: label, status, summary: detail },
              ]
        );
      } else {
        this.setToolCalls(
          existing
            ? this.toolCalls.map((tc) =>
                tc.task_id === callId
                  ? { ...tc, description: label || tc.description, summary: detail ?? tc.summary }
                  : tc
              )
            : [
                ...this.toolCalls,
                { task_id: callId, agent_name: toolName, kind: "tool", description: label, status: "running", summary: detail, started_at: Date.now() },
              ]
        );
      }
      return;
    }

    const entry: ToolCallEntry = {
      task_id: crypto.randomUUID(),
      agent_name: toolName,
      kind: "tool",
      description: label,
      status: "running",
      summary: detail,
    };
    this.setToolCalls([
      ...this.toolCalls.map((tc) =>
        tc.status === "running" && tc.kind !== "agent" && tc.kind !== "planner"
          ? { ...tc, status: "done" as const }
          : tc
      ),
      entry,
    ]);
  }

  private onSkillLoaded(skillName: string, callId?: string): void {
    if (!skillName) return;
    const description = this.host.translate("app.agents.loadingSkill", { name: skillName });
    const toolName = this.host.translate("app.agents.tool");
    if (callId) {
      const existing = this.toolCalls.some((tc) => tc.task_id === callId);
      this.setToolCalls(
        existing
          ? this.toolCalls.map((tc) =>
              tc.task_id === callId
                ? { ...tc, description, skill_name: skillName, status: "running" as const }
                : tc
            )
          : [
              ...this.toolCalls,
              { task_id: callId, agent_name: toolName, kind: "tool", description, status: "running", skill_name: skillName },
            ]
      );
      return;
    }
    this.setToolCalls([
      ...this.toolCalls.map((tc) =>
        tc.status === "running" && tc.kind !== "agent" && tc.kind !== "planner"
          ? { ...tc, status: "done" as const }
          : tc
      ),
      { task_id: crypto.randomUUID(), agent_name: toolName, kind: "tool", description, status: "running", skill_name: skillName },
    ]);
  }

  private onInjected(items: ChatInjection[]): void {
    if (items.length === 0) return;
    const delivered = new Set(items.map((i) => i.id));
    this.pendingInjections = this.pendingInjections.filter((i) => !delivered.has(i.id));
    this.injections = [...this.injections, ...items];
    this.messages = this.messages.map((m) =>
      m.id === this.assistantId
        ? { ...m, injections: [...(m.injections ?? []), ...items] }
        : m
    );
    this.emit();
  }

  private upsertAgentRow(taskId: string, patch: Partial<ToolCallEntry>): void {
    if (!taskId) return;
    const existing = this.toolCalls.some((tc) => tc.task_id === taskId);
    if (existing) {
      this.setToolCalls(
        this.toolCalls.map((tc) => (tc.task_id === taskId ? { ...tc, ...patch } : tc))
      );
    } else {
      this.setToolCalls([
        ...this.toolCalls,
        {
          task_id: taskId,
          agent_name: patch.agent_name || "agent",
          kind: "agent",
          description: patch.description || taskId,
          status: "running",
          ...patch,
        },
      ]);
    }
  }

  private appendAgentSummary(taskId: string, token: string): void {
    this.setToolCalls(
      this.toolCalls.map((tc) => {
        if (tc.task_id !== taskId || tc.kind !== "agent") return tc;
        const summary = ((tc.summary ?? "") + token).slice(-1200);
        return { ...tc, summary, thinking: undefined };
      })
    );
  }

  // ── Settlement ────────────────────────────────────────────────────────────

  private settleTurnStats(): TurnUsageMeta | undefined {
    const stats = this.stats;
    this.stats = null;
    if (!stats || stats.prompt + stats.completion <= 0) return undefined;
    const meta: TurnUsageMeta = {
      duration_ms: Math.max(0, Date.now() - stats.started_at),
      prompt_tokens: stats.prompt,
      completion_tokens: stats.completion,
      total_tokens: stats.total,
    };
    if (stats.cache_reported && stats.prompt > 0) {
      meta.cached_tokens = stats.cached;
      meta.hit_ratio = Math.min(1, Math.max(0, stats.cached / stats.prompt));
    }
    return meta;
  }

  private settle(status: "done" | "error" | "stopped", err?: string): void {
    if (this.settled) return;
    this.settled = true;

    const turnUsage = this.settleTurnStats();
    const isError = status === "error";
    const finalToolCalls = this.toolCalls.map((e) => {
      if (e.status !== "running") return e;
      if (e.kind === "agent") {
        return { ...e, status: "skipped" as const, thinking: undefined, confirm: undefined };
      }
      return isError
        ? { ...e, status: "error" as const, confirm: undefined }
        : { ...e, status: "done" as const };
    });
    this.content = stripControlMarkers(this.content);
    if (status === "stopped") {
      const suffix = "\n\n" + this.host.translate("app.generationStopped");
      if (!this.content.endsWith(this.host.translate("app.generationStopped"))) {
        this.content = this.content ? this.content + suffix : this.content;
      }
    }

    // A stopped turn notifies nothing — the user pressed Stop themselves.
    // Done / error settle with the desktop card, like the foreground does.
    if (status !== "stopped") {
      const notifyLine =
        this.content
          .split("\n")
          .find((line) => line.trim() && !line.trim().startsWith("[System]")) ?? "";
      this.host.notifySettled(!isError, isError ? err ?? "" : notifyLine);
    }

    const deliveredInjections = this.injections;
    // Mirror the foreground: a completed turn always persists; a stopped or
    // failed one only when it produced something (content, reasoning or
    // settled tool rows) — an empty failure leaves no phantom row behind.
    const shouldPersist =
      status === "done" ||
      this.content.trim() ||
      this.reasoning.trim() ||
      finalToolCalls.length > 0;
    if (shouldPersist) {
      saveHistory(
        this.sessionId,
        "assistant",
        this.content,
        finalToolCalls.length > 0 ? JSON.stringify(finalToolCalls) : undefined,
        this.reasoning || undefined,
        undefined,
        deliveredInjections.length > 0 ? JSON.stringify(deliveredInjections) : undefined,
        isError ? err : undefined,
        isError ? "error" : status === "stopped" ? "stopped" : undefined,
        turnUsage
      )
        .then((dbId) => {
          this.messages = this.messages.map((m) =>
            m.id === this.assistantId ? { ...m, dbId } : m
          );
          this.emit();
        })
        .catch(console.error);
    }

    this.messages = this.messages
      .filter((m) => !m.id.startsWith("agent-progress-"))
      .map((m) =>
        m.id === this.assistantId
          ? {
              ...m,
              content: this.content,
              streaming: false,
              ...(isError ? { error: err, status: "error" as const } : {}),
              ...(status === "stopped" ? { status: "stopped" as const } : {}),
              ...(finalToolCalls.length ? { tool_calls: finalToolCalls } : {}),
              ...(turnUsage ? { usage_meta: turnUsage } : {}),
            }
          : m
      );
    this.toolCalls = finalToolCalls;
    this.pendingConfirm = null;
    this.host.onConfirm(this.sessionId, null);

    // Whatever never reached a request is sent as the next user turn.
    if (this.pendingInjections.length > 0) {
      this.host.onUndelivered(
        this.sessionId,
        this.pendingInjections.map((i) => ({ text: i.text }))
      );
      this.pendingInjections = [];
    }

    this.unlisteners.forEach((fn) => fn());
    this.unlisteners = [];
    this.emit();
    for (const cb of this.settledListeners) cb(this.messages);
    this.transcriptListeners.clear();
    this.settledListeners.clear();
  }
}