/**
 * `/skill` mentions in the composer (Claude-Code style slash commands).
 *
 * Grammar (one mention, `/` must start a word):
 *
 * ```text
 * /<skill name>
 * ```
 *
 * The token stays in the message body — the model then sees *which* skill was
 * referenced and resolves it with `use_skill`. Unlike `@path` there is nothing
 * to materialise on send: a skill name is a reference, not content.
 */
import type { Skill } from "./types";

/** One row of the `/skill` popup. */
export interface SkillMentionOption {
  /** Skill name — the `use_skill` key, and the token that gets inserted. */
  name: string;
  description: string;
}

/** Map the workspace skill list onto popup rows, alphabetical by name. */
export function toSkillOptions(skills: Skill[]): SkillMentionOption[] {
  return skills
    .map((skill) => ({ name: skill.name, description: skill.description ?? "" }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * The `/skill` token that the caret currently sits in, if any.
 *
 * Same contract as `activeMentionQuery`: the token must start at a word
 * boundary and must not yet contain whitespace, `/`, `@` or `#`. That keeps a
 * typed path (`src/App.css`), an `@file` token and the `#` of a line range from
 * opening the popup — and makes the `@` and `/` popups mutually exclusive, so
 * only one of them can ever be open.
 */
export function activeSlashQuery(
  text: string,
  caret: number,
): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const slash = before.lastIndexOf("/");
  if (slash < 0) return null;
  const previous = slash === 0 ? "" : before[slash - 1];
  if (previous && !/\s/.test(previous)) return null;
  const query = before.slice(slash + 1);
  if (/[\s/@#]/.test(query)) return null;
  return { start: slash, query };
}

/** Popup ordering: name-prefix matches first, then name, then description. */
export function scoreSkillOption(option: SkillMentionOption, query: string): number | null {
  const needle = query.toLowerCase();
  const name = option.name.toLowerCase();
  if (needle === "") return 3;
  if (name.startsWith(needle)) return 0;
  if (name.includes(needle)) return 1;
  if (option.description.toLowerCase().includes(needle)) return 2;
  return null;
}
