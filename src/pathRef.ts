/**
 * `&path` references in the composer — a workspace file named in the message
 * **without** its content travelling to the model.
 *
 * Grammar (the reference must start a word):
 *
 * ```text
 * &<workspace-relative path>
 * ```
 *
 * This is the sibling of the `@` mention (`fileMention.ts`) and shares its
 * popup, ranking and workspace-file cache, but nothing beyond that: an `@`
 * token promises the bytes are attached (`parseFileMentions` →
 * `resolveFileMentions` → an attachment or a content part), while `&` promises
 * they are *not*. Concretely, a `&path` token
 *
 * - never reaches the attachment pipeline: `MENTION_RE` in `fileMention.ts`
 *   only matches `@`, so `resolveFileMentions` cannot see it and the request
 *   the backend builds carries no `file` / `image_url` part for it;
 * - cannot carry a `#12-24` range — there is nothing to slice, so the ranges
 *   `@` supports would only be a lie;
 * - is not filtered by the selected model's capabilities: a model that cannot
 *   *read* a format can still be told the path and parse the file with a tool.
 *
 * The token stays in the body verbatim, so the model sees which file is meant
 * and decides itself whether to open it.
 */

/**
 * Characters that end a `&path` token, and the reason a path holding one of
 * them cannot be referenced at all.
 *
 * Whitespace separates tokens like anywhere else in a message; `&` / `@` / `#`
 * / `<` / `>` are the triggers and delimiters of the other composer grammars
 * (and of the transcript's token highlighting). `/` is deliberately *not* here:
 * every path contains it.
 */
export const PATH_REF_STOP = /[\s&@#<>]/;

/** `&<path>` — the token inserted when a popup row is accepted. */
export function pathRefTokenFor(path: string): string {
  return `&${path}`;
}

/**
 * Whether `path` can be expressed as a `&path` token at all.
 *
 * A path holding whitespace (or any other stop character) would be cut in half
 * by the tokenizer — `&my file.md` reads as the path `my` and then prose — so
 * such files are left out of the popup instead of being offered as a reference
 * that resolves to the wrong file.
 */
export function isPathRefExpressible(path: string): boolean {
  return path.length > 0 && !PATH_REF_STOP.test(path);
}

/**
 * The `&path` token the caret currently sits in, if any — the popup's opening
 * condition, mirroring `activeMentionQuery`.
 *
 * The `&` must open the line or follow whitespace, so `R&D`, `A & B` and a
 * shell's `&&` never open the popup; the token ends at whitespace or at one of
 * `PATH_REF_STOP`'s characters.
 */
export function activePathRefQuery(
  text: string,
  caret: number,
): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const at = before.lastIndexOf("&");
  if (at < 0) return null;
  const previous = at === 0 ? "" : before[at - 1];
  if (previous && !/\s/.test(previous)) return null;
  const query = before.slice(at + 1);
  // A second `&` is `&&`, not a reference — and a space means the reference
  // was already finished one word ago.
  if (PATH_REF_STOP.test(query)) return null;
  return { start: at, query };
}
