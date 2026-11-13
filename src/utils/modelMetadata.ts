/**
 * Matching of remote model ids against the merged model-metadata catalogue
 * (`models.dev` + OpenRouter + `basellm/llm-metadata`, see
 * `src-tauri/src/model_metadata.rs`).
 *
 * A provider's `/models` listing and the catalogue frequently disagree on the
 * exact name: gateways (new-api / one-api / LiteLLM / OpenRouter / Bedrock)
 * prefix a vendor, append a date stamp or a variant suffix, and upstream
 * renames models over time. The catalogue stores normalised names (lowercase,
 * flattened — the Rust merge writes `normalizeModelName`'s output back into
 * `model_name`), so matching normalises the remote id once and:
 *
 * 1. an equal normalised name is a direct hit and scores `1.0` — the common
 *    case, and a map lookup in `matchModelCatalog`;
 * 2. ids the catalogue does not know fall through to a similarity score, and
 *    the closest entry above a floor wins:
 *     - a name that is a contiguous token run of the other one
 *       (`llama-4-scout-17b-16e-instruct` inside `cerebras-llama-4-scout-...`)
 *       scores `0.65 ... 0.95`, scaled by how much of the longer name it
 *       covers;
 *     - otherwise a blend of token Dice coefficient (order-insensitive word
 *       overlap) and character-level similarity (edit distance);
 *     - a modality mismatch (image / video / speech / embedding model vs chat
 *       model) halves whatever the name similarity produced.
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

/**
 * Similarity (0-1) between a remote model id and a catalogue name.
 *
 * The catalogue stores normalised names and `normalizeModelName` is
 * idempotent, so passing an already-normalised name through again is safe —
 * which keeps this scorer usable against `config.model_metadata` entries
 * persisted before the catalogue was flattened.
 */
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
 * Scored fallback of `matchModelMetadata`: no entry shares the id's normalised
 * name, so keep the closest one above `MIN_MATCH_SCORE`. Ties prefer the entry
 * whose normalised name is closest in length.
 */
function fallbackMatch(
    modelId: string,
    normalizedQuery: string,
    entries: readonly ModelMetadata[],
): ModelMetadata | null {
    let best: ModelMetadata | null = null;
    let bestScore = 0;
    let bestLengthDelta = Number.POSITIVE_INFINITY;

    for (const entry of entries) {
        const score = modelNameSimilarity(modelId, entry.model_name);
        if (score < MIN_MATCH_SCORE) continue;

        const lengthDelta = Math.abs(
            normalizeModelName(entry.model_name).length - normalizedQuery.length,
        );

        if (score > bestScore || (score === bestScore && lengthDelta < bestLengthDelta)) {
            best = entry;
            bestScore = score;
            bestLengthDelta = lengthDelta;
        }
    }

    return best ? { ...best, match_score: bestScore } : null;
}

/**
 * Pick the catalogue entry for `modelId`.
 *
 * Phase 1 is the direct hit: the stored name is already normalised, so an
 * equal normalised name is a full match (score `1.0`). Phase 2 — only for ids
 * the catalogue does not know — is `fallbackMatch`, which keeps renamed models
 * (`deepseek-chat-v3.1` → `deepseek-chat`) matched without guessing wildly.
 *
 * Returns a copy of the winning entry with `match_score` set (ready to persist
 * in `AppConfig.model_metadata`), or `null` when nothing matches.
 */
export function matchModelMetadata(
    modelId: string,
    entries: readonly ModelMetadata[],
): ModelMetadata | null {
    const normalizedQuery = normalizeModelName(modelId);
    if (!normalizedQuery) return null;

    for (const entry of entries) {
        if (normalizeModelName(entry.model_name) === normalizedQuery) {
            return { ...entry, match_score: 1 };
        }
    }

    return fallbackMatch(modelId, normalizedQuery, entries);
}

/**
 * Match a whole model catalogue in one pass.
 *
 * The catalogue holds one entry per normalised name, so the common case — the
 * id exists in it, behind a gateway prefix / date stamp / variant suffix — is
 * a map lookup after one normalisation of the id; only the misses reach the
 * similarity scorer. Unmatched ids are simply absent from the result.
 */
export function matchModelCatalog(
    modelIds: readonly string[],
    entries: readonly ModelMetadata[],
): Record<string, ModelMetadata> {
    const matched: Record<string, ModelMetadata> = {};
    if (entries.length === 0) return matched;

    const byNormalizedName = new Map<string, ModelMetadata>();
    for (const entry of entries) {
        const key = normalizeModelName(entry.model_name);
        if (key && !byNormalizedName.has(key)) byNormalizedName.set(key, entry);
    }

    for (const id of modelIds) {
        const normalizedQuery = normalizeModelName(id);
        const exact = normalizedQuery ? byNormalizedName.get(normalizedQuery) : undefined;
        if (exact) {
            matched[id] = { ...exact, match_score: 1 };
        } else if (normalizedQuery) {
            const match = fallbackMatch(id, normalizedQuery, entries);
            if (match) matched[id] = match;
        }
    }
    return matched;
}

/**
 * Is this a DeepSeek-family model? Mirrors `is_deepseek_model` in
 * `src-tauri/src/llm_complete.rs`: such models always get DS-Format, so the
 * Settings switch is shown locked-on for them. Gateways prefix the vendor
 * (`deepseek/deepseek-chat`) and change the casing, hence the loose match;
 * the catalogue vendor catches renamed ids.
 */
export function isDeepSeekModel(
    modelId: string | undefined,
    metadata?: ModelMetadata,
): boolean {
    if ((modelId ?? "").toLowerCase().includes("deepseek")) return true;
    return (metadata?.vendor ?? "").toLowerCase().includes("deepseek");
}

