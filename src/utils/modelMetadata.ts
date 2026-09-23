/**
 * Matching of remote model ids against the `basellm/llm-metadata` catalogue.
 *
 * A provider's `/models` listing and the catalogue frequently disagree on the
 * exact name: gateways (new-api / one-api / LiteLLM / OpenRouter / Bedrock)
 * prefix a vendor, append a date stamp or a variant suffix, and upstream
 * renames models over time. So instead of requiring an exact id we score every
 * catalogue entry against the id and keep the closest one above a similarity
 * floor.
 *
 * Scores are computed on a normalised form (see `normalizeModelName`):
 *
 * 1. identical normalised names score `1.0`;
 * 2. a name that is a contiguous token run of the other one
 *    (`llama-4-scout-17b-16e-instruct` inside `cerebras-llama-4-scout-...`)
 *    scores `0.65 ... 0.95`, scaled by how much of the longer name it covers;
 * 3. otherwise a blend of token Dice coefficient (order-insensitive word
 *    overlap) and character-level similarity (edit distance);
 * 4. a modality mismatch (image / video / speech / embedding model vs chat
 *    model) halves whatever the name similarity produced.
 *
 * Anything below `MIN_MATCH_SCORE` is treated as "no match" — reporting no
 * capabilities is better than reporting the wrong ones.
 */
import type { ModelMetadata } from "../types";

/** Below this score an entry is considered unrelated to the model id. */
export const MIN_MATCH_SCORE = 0.6;

/**
 * Tokens that carry no model identity, e.g. serving/quantisation markers and
 * release-channel words. Both sides are stripped, so `gpt-4o-latest` still
 * resolves to the `gpt-4o` catalogue entry.
 */
const DECORATION_TOKENS = new Set([
    "latest",
    "preview",
    "exp",
    "experimental",
    "gguf",
    "awq",
    "gptq",
    "mlx",
    "fp8",
    "fp16",
    "int8",
    "int4",
]);

/**
 * Tokens that mark a non-chat model class. Image / video / speech / embedding
 * models often share a name prefix with a chat model (`gemini-3-pro-image` vs
 * `gemini-3-pro`) but report completely unrelated capabilities.
 *
 * Vision suffixes are handled separately in `modalityOf`, because they glue
 * themselves to version numbers (`glm-4.5v` tokenises to `5v`).
 */
const MODALITY_TOKENS = new Set([
    "image",
    "imagen",
    "video",
    "veo",
    "sora",
    "tts",
    "audio",
    "speech",
    "voice",
    "whisper",
    "realtime",
    "embed",
    "embedding",
    "embeddings",
    "rerank",
    "moderation",
    "diffusion",
    "flux",
    "dall",
]);

/**
 * Reduce a model id to a comparable, hyphen-separated token string.
 *
 * Handles the decorations seen in the wild:
 * - provider namespaces and router prefixes: `openai/gpt-4o`,
 *   `@cf/meta/llama-3-8b`, `accounts/fireworks/models/llama-v3p3-70b-instruct`
 * - cloud-vendor prefixes: `meta.llama3-70b-instruct-v1:0`,
 *   `amazon.nova-pro-v1:0`
 * - variant suffixes: `gpt-4o:free`, `llama-3-8b:nitro`
 * - API revision stamps: `gpt-4o-mini-2024-07-18`,
 *   `claude-3-5-sonnet-20241022`, `gpt-3.5-turbo-0613`
 * - separator differences: dots, underscores and spaces become token
 *   boundaries, so `gemini-2.5-pro` and `gemini-2-5-pro` normalise identically
 * - serving and quantisation markers (`-gguf`, `-fp8`, `-latest`, ...)
 */
export function normalizeModelName(raw: string): string {
    let name = raw.trim().toLowerCase();

    // Keep only the model part of a namespaced id.
    const slash = name.lastIndexOf("/");
    if (slash >= 0) name = name.slice(slash + 1);

    // Bedrock-style `<vendor>.<model>`, but only when the prefix is a bare
    // vendor name so dotted versions such as `qwen2.5-72b` stay intact.
    const dot = name.indexOf(".");
    if (dot > 0 && /^[a-z]+$/.test(name.slice(0, dot))) name = name.slice(dot + 1);

    const colon = name.indexOf(":");
    if (colon >= 0) name = name.slice(0, colon);

    name = name
        .replace(/-\d{4}-\d{2}-\d{2}$/, "") // -2024-07-18
        .replace(/-\d{8}$/, "") // -20241022
        .replace(/-\d{4}$/, ""); // -0613 / -1106

    const tokens = name
        .replace(/[^a-z0-9]+/g, "-")
        .split("-")
        .filter((token) => token && !DECORATION_TOKENS.has(token));

    return tokens.join("-");
}

/**
 * Modality signature of a normalised name: `""` for a plain chat model,
 * otherwise the sorted modality tokens (e.g. `"vision"`, `"image"`).
 *
 * A bare `v` is only trusted as the final token, and `vl` / `<digits>v` count
 * wherever they appear.
 */
function modalityOf(tokens: readonly string[]): string {
    const found: string[] = [];
    tokens.forEach((token, index) => {
        const isVisionSuffix =
            token === "vl" ||
            (token === "v" && index === tokens.length - 1) ||
            /^\d+v$/.test(token);

        if (isVisionSuffix) {
            found.push("vision");
            return;
        }
        if (MODALITY_TOKENS.has(token)) found.push(token);
    });
    return Array.from(new Set(found)).sort().join("+");
}

/** Sorensen-Dice coefficient over token multisets (`a` is order-insensitive). */
function diceCoefficient(a: readonly string[], b: readonly string[]): number {
    if (a.length === 0 || b.length === 0) return 0;

    const remaining = new Map<string, number>();
    for (const token of a) remaining.set(token, (remaining.get(token) ?? 0) + 1);

    let shared = 0;
    for (const token of b) {
        const count = remaining.get(token) ?? 0;
        if (count > 0) {
            remaining.set(token, count - 1);
            shared += 1;
        }
    }
    return (2 * shared) / (a.length + b.length);
}

/** `true` when `needle` appears as a consecutive run inside `haystack`. */
function containsTokenRun(haystack: readonly string[], needle: readonly string[]): boolean {
    if (needle.length === 0 || needle.length > haystack.length) return false;

    for (let start = 0; start + needle.length <= haystack.length; start++) {
        let matched = true;
        for (let offset = 0; offset < needle.length; offset++) {
            if (haystack[start + offset] !== needle[offset]) {
                matched = false;
                break;
            }
        }
        if (matched) return true;
    }
    return false;
}

/** Levenshtein distance with a rolling row (model names are short). */
function levenshtein(a: string, b: string): number {
    if (a === b) return 0;
    if (a.length === 0) return b.length;
    if (b.length === 0) return a.length;

    let previous = Array.from({ length: b.length + 1 }, (_, i) => i);
    let current = new Array<number>(b.length + 1);

    for (let i = 1; i <= a.length; i++) {
        current[0] = i;
        for (let j = 1; j <= b.length; j++) {
            const cost = a[i - 1] === b[j - 1] ? 0 : 1;
            current[j] = Math.min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + cost);
        }
        [previous, current] = [current, previous];
    }
    return previous[b.length];
}

/** Similarity (0-1) between a remote model id and a catalogue name. */
export function modelNameSimilarity(remoteId: string, candidateName: string): number {
    const query = normalizeModelName(remoteId);
    const candidate = normalizeModelName(candidateName);
    if (!query || !candidate) return 0;
    if (query === candidate) return 1;

    const queryTokens = query.split("-");
    const candidateTokens = candidate.split("-");

    let score: number;

    // A name that appears verbatim inside the other one is a strong signal: the
    // remote id is usually the catalogue name plus (or minus) a vendor prefix.
    const [shorter, longer] =
        queryTokens.length <= candidateTokens.length
            ? [queryTokens, candidateTokens]
            : [candidateTokens, queryTokens];

    if (shorter.length >= 2 && containsTokenRun(longer, shorter)) {
        score = 0.65 + 0.3 * (shorter.length / longer.length);
    } else {
        const dice = diceCoefficient(queryTokens, candidateTokens);
        const charSimilarity =
            1 - levenshtein(query, candidate) / Math.max(query.length, candidate.length);
        score = 0.55 * dice + 0.45 * charSimilarity;
    }

    // A media / embedding / speech model shares a name prefix with its chat
    // sibling but not its capabilities, so never let those win on the name.
    if (modalityOf(queryTokens) !== modalityOf(candidateTokens)) score *= 0.5;

    return score;
}

/**
 * Pick the catalogue entry closest to `modelId`.
 *
 * Returns a copy of the winning entry with `match_score` set (ready to persist
 * in `AppConfig.model_metadata`), or `null` when nothing scores at least
 * `MIN_MATCH_SCORE`. Ties prefer an exact raw-name match, then the entry whose
 * normalised name is closest in length.
 */
export function matchModelMetadata(
    modelId: string,
    entries: readonly ModelMetadata[],
): ModelMetadata | null {
    const rawQuery = modelId.trim().toLowerCase();
    const normalizedQuery = normalizeModelName(modelId);

    let best: ModelMetadata | null = null;
    let bestScore = 0;
    let bestLengthDelta = Number.POSITIVE_INFINITY;
    let bestExact = false;

    for (const entry of entries) {
        const score = modelNameSimilarity(modelId, entry.model_name);
        if (score < MIN_MATCH_SCORE) continue;

        const exact = entry.model_name.trim().toLowerCase() === rawQuery;
        const lengthDelta = Math.abs(
            normalizeModelName(entry.model_name).length - normalizedQuery.length,
        );

        const better =
            score > bestScore ||
            (score === bestScore &&
                ((exact && !bestExact) || (exact === bestExact && lengthDelta < bestLengthDelta)));

        if (better) {
            best = entry;
            bestScore = score;
            bestLengthDelta = lengthDelta;
            bestExact = exact;
        }
    }

    return best ? { ...best, match_score: bestScore } : null;
}

/**
 * Match a whole model catalogue in one pass. Unmatched ids are simply absent
 * from the result.
 */
export function matchModelCatalog(
    modelIds: readonly string[],
    entries: readonly ModelMetadata[],
): Record<string, ModelMetadata> {
    const matched: Record<string, ModelMetadata> = {};
    if (entries.length === 0) return matched;

    for (const id of modelIds) {
        const match = matchModelMetadata(id, entries);
        if (match) matched[id] = match;
    }
    return matched;
}
