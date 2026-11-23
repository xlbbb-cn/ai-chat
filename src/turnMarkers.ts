/**
 * Answer-level control markers shared by the foreground transcript and the
 * background turn driver.
 *
 * The goal-loop end marker (`[goal:DONE]`) and the external-wait marker
 * (`[WAIT_EVT: reason]`) are backend control signals: the main loop reads them
 * from the streamed answer to decide whether the turn may end, but they must
 * never reach the transcript bubble or the persisted row.
 * Keep in sync with `GOAL_DONE_MARKER` / `WAIT_EVT_PREFIX` in
 * `src-tauri/src/llm_complete.rs`.
 */
export const GOAL_DONE_MARKER = "[goal:DONE]";
export const WAIT_EVT_MARKER = "[WAIT_EVT:";

/**
 * Remove every control marker and the blank tail it leaves behind.
 * `[WAIT_EVT: …]` is variable length, so each complete occurrence is cut up to
 * its closing bracket; an unterminated one (still streaming) stays in place.
 */
export function stripControlMarkers(text: string): string {
  let next = text.includes(GOAL_DONE_MARKER)
    ? text.split(GOAL_DONE_MARKER).join("")
    : text;
  let start = next.indexOf(WAIT_EVT_MARKER);
  while (start !== -1) {
    const close = next.indexOf("]", start + WAIT_EVT_MARKER.length);
    if (close === -1) break;
    next = next.slice(0, start) + next.slice(close + 1);
    start = next.indexOf(WAIT_EVT_MARKER);
  }
  return next.trimEnd();
}

/**
 * Live-stream variant of {@link stripControlMarkers}: both markers are written
 * as the last thing in the answer, so only one that just completed at the tail
 * is trimmed. That keeps the per-token cost flat instead of rescanning the
 * whole accumulator on every token.
 */
export function stripTrailingControlMarkers(text: string): string {
  let next = text;
  if (next.endsWith(GOAL_DONE_MARKER)) {
    next = next.slice(0, -GOAL_DONE_MARKER.length);
  }
  if (next.endsWith("]")) {
    const start = next.lastIndexOf(WAIT_EVT_MARKER);
    if (
      start !== -1 &&
      next.indexOf("]", start + WAIT_EVT_MARKER.length) === next.length - 1
    ) {
      next = next.slice(0, start).trimEnd();
    }
  }
  return next;
}