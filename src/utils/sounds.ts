/**
 * The two UI sound effects, bundled by Vite as hashed assets and played
 * through the webview's own audio — no plugin, no extra process.
 *
 * - `info`: the light message cue — a turn that finished while the user was
 *   looking at the window (the notification card only exists for the
 *   background case)
 * - `completed`: the session-successfully-finished cue — a turn that finished
 *   in the background, together with the notification card
 *
 * Both come from `src/assets/*.mp3` and are controlled by the titlebar `⋯`
 * menu's sound switch (`sound_effects` in the config); this module only
 * knows how to play them.
 */

import completedSound from "../assets/completed.mp3";
import infoSound from "../assets/infomation.mp3";

export type SoundKind = "info" | "completed";

/** One reusable element per effect — re-playing resets the cursor instead of
 * re-creating the decoder, so two turns in a row do not stack elements. */
let infoAudio: HTMLAudioElement | null = null;
let completedAudio: HTMLAudioElement | null = null;

const VOLUME = 0.6;

/** Fire and forget: playback failure (autoplay policy on a webview that has
 * never seen a gesture, a missing decoder) must never surface as an error. */
export function playSound(kind: SoundKind) {
    const element =
        kind === "info"
            ? (infoAudio ??= Object.assign(new Audio(infoSound), { volume: VOLUME }))
            : (completedAudio ??= Object.assign(new Audio(completedSound), { volume: VOLUME }));
    try {
        element.currentTime = 0;
    } catch {
        /* still loading — the reset is best-effort */
    }
    void element.play().catch(() => {
        /* no gesture, throttled webview — the cue is simply skipped */
    });
}
