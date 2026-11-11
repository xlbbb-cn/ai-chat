/**
 * `+agent` mentions in the composer.
 *
 * A mention embeds a sub-agent's **system prompt** into a new conversation.
 * Unlike `/skill`, the token is not a reference the model resolves itself: the
 * backend snapshots the agent definition on the session's first send and
 * appends the prompt to the main system prompt for the whole session.
 *
 * Grammar (the `+` must open a line, and the line may only hold `+token`
 * words):
 *
 * ```text
 * +<agent token>
 * +<agent token> +<another token>
 * ```
 *
 * Requiring the line to start with `+` is what keeps ordinary arithmetic
 * (`2 + 2`) and `C++` from opening the popup. The token stays in the message
 * body so the transcript records which role shaped the answer.
 */
import type { AgentMention } from "./types";

/** One row of the `+agent` popup. */
export type AgentMentionOption = AgentMention;

/**
 * The `+agent` token the caret currently sits in, if any.
 *
 * `start` is the index of the `+` that opened the popup — accepting an item
 * replaces the whole `+query` fragment. The line prefix may hold earlier
 * `+token` words, so `+a +b` can be typed without closing and reopening the
 * popup; anything else on the line (including the trailing space after an
 * accepted token) closes it.
 */
export function activeAgentQuery(
  text: string,
  caret: number,
): { start: number; query: string } | null {
  const lineStart = caret > 0 ? text.lastIndexOf("\n", caret - 1) + 1 : 0;
  const line = text.slice(lineStart, caret);
  const match = /^[ \t]*(?:\+[^\s+@/<>#]+\s+)*\+([^\s+@/<>#]*)$/.exec(line);
  if (!match) return null;
  const query = match[1];
  return { start: caret - query.length - 1, query };
}

/** Popup ordering: name/token prefix first, then contains, then description. */
export function scoreAgentMention(option: AgentMentionOption, query: string): number | null {
  const needle = query.toLowerCase();
  const name = option.name.toLowerCase();
  const token = option.token.toLowerCase();
  const description = option.description.toLowerCase();
  if (needle === "") return 3;
  if (name.startsWith(needle) || token.startsWith(needle)) return 0;
  if (name.includes(needle) || token.includes(needle)) return 1;
  if (description.includes(needle)) return 2;
  return null;
}

/**
 * Whether the draft still contains `+<token>` as a whole word.
 *
 * The popup records the selection, but the user may delete the token before
 * sending; only tokens that survived into the body may embed a role. The
 * trailing punctuation set covers both ASCII and CJK sentence endings, because
 * `+嵌入式固件工程师，请…` is a mention followed by prose.
 */
export function hasAgentToken(text: string, token: string): boolean {
  if (!token) return false;
  const escaped = token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(
    `(^|\\s)\\+${escaped}(?=$|[\\s,.;:!?，。；：！？、)}\\]]|$)`,
    "u",
  );
  return pattern.test(text);
}
