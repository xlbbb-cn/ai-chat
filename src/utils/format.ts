/**
 * Value → display string helpers shared across panels.
 *
 * These used to be private copies in two or three components, and the copies
 * had drifted (one rounded sizes to whole numbers, one kept a decimal; two
 * `escapeHtml` variants that were one regex apart), so the same byte count
 * could read differently on two screens.
 */

const SIZE_UNITS = ["B", "KB", "MB", "GB"] as const;

/** Human-readable byte size: `512 B`, `4 KB`, `12.3 MB`, `1.2 GB`. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < SIZE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text = value >= 10 || unit === 0 ? Math.round(value).toString() : value.toFixed(1);
  return `${text} ${SIZE_UNITS[unit]}`;
}

/** `published_at` style ISO timestamp as a locale date; empty when unusable. */
export function formatDate(iso: string | undefined, locale: string): string {
  if (!iso) return "";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString(locale);
}

/** Escape the five HTML-significant characters for hand-built markup. */
export function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
