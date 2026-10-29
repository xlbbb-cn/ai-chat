/**
 * Countdown label for the delay timers and the sleep-lock badge: `mm:ss`, or
 * `h:mm:ss` past an hour. Clamped at zero — the row that shows it disappears
 * as soon as the timer fires.
 */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const seconds = total % 60;
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  const pad = (value: number) => String(value).padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${pad(minutes)}:${pad(seconds)}`;
}
