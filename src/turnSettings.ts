/**
 * Per-conversation model settings: the model and thinking depth a conversation
 * is (or was last) driven with.
 *
 * `App.tsx` keeps a single `selectedModel` for the whole window, so a
 * conversation whose turn kept running in the background showed whatever the
 * *other* conversation had just picked — while the turn itself went on using
 * its own model. The backend already knows that model (it freezes it per turn)
 * and already publishes it through `turn-state`, `list_active_turns` and
 * `get_turn_snapshot`; this module is where the frontend keeps it per session.
 *
 * Precedence when a conversation becomes visible, most authoritative first:
 *   1. the turn running right now (the backend's frozen values);
 *   2. what this conversation recorded (its turn may have ended already);
 *   3. the current global selection (a new chat, a session from a past run).
 *
 * Invariants:
 * - `selectedModel` / `reasoningEfforts` stay the runtime truth: a record only
 *   writes back into them when the visible conversation changes.
 * - A running turn cannot be re-parameterised, so its values win until it ends.
 * - Nothing here is persisted. The toolbar's model choice never was (only the
 *   Settings panel saves `config.model`), and the depth is already a per-model
 *   config value keyed by model id — restoring the model restores its depth.
 */

/** One conversation's model settings. */
export interface TurnSettings {
  /** Model id — the value domain of `selectedModel`. */
  model: string;
  /** Thinking depth; `""` = provider default (same empty semantics as
   *  `reasoningEfforts`). */
  effort: string;
}

/** The settings a visible conversation should show. */
export function pickTurnSettings(
  live: TurnSettings | undefined,
  sticky: TurnSettings | undefined,
  fallback: TurnSettings
): TurnSettings {
  return live ?? sticky ?? fallback;
}
