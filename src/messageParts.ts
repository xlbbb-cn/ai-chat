/**
 * One user message's `content` payload: text plus the content parts its
 * attachments become.
 *
 * This is the only place an attachment turns into a part, and it holds one
 * invariant: **a `file` part is never produced.** The sendable forms are text,
 * `image_url` and `input_audio`, and nothing else.
 *
 * Providers of the "text and images only" kind answer a `file` part with a bare
 * `400 invalid_request_error … file content parts are not supported here; send
 * the image as an image_url part`, which kills the whole turn — so the part type
 * is decided by the *payload* (a `data:image/…` URL) rather than by the
 * extension-derived `Attachment["kind"]`, and a binary with no sendable form is
 * named in a text stub instead of being sent. A catalogue entry that advertises
 * file input is *not* a licence to send one: the gateway in front of the model
 * refused it anyway, which is how this invariant was learned.
 */
import { extensionOf } from "./attachments";
import { mentionDisplayName } from "./composerFiles";
import type { Attachment, ContentPart, MessageContent } from "./types";

/** Strip the `data:<mime>;base64,` prefix — `input_audio` wants the payload. */
function dataUrlBase64(dataUrl: string): string {
  const comma = dataUrl.indexOf(",");
  return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
}

/** `true` when a `data:` URL carries an image, whatever the file name said. */
function isImageDataUrl(dataUrl: string | undefined): boolean {
  if (!dataUrl) return false;
  return /^data:image\/[a-z0-9.+-]+[;,]/i.test(dataUrl);
}

/**
 * Code-fence language for an inlined text attachment.
 *
 * The name may carry a range (`kk.pdf#2`, `src/App.tsx#12-24`), which would
 * otherwise leak into the fence tag, and a converted PDF mention is Markdown
 * no matter what its extension says.
 */
function fenceLanguage(att: Attachment): string {
  if (att.mime === "text/markdown") return "markdown";
  const ext = extensionOf(att.name.split("#")[0]);
  return /^[a-z0-9]+$/.test(ext) ? ext : "";
}

/** Model-facing note for a file whose bytes could not be sent. */
function unsentFileNote(att: Attachment, reason: string): string {
  return `\n[Attached file: ${mentionDisplayName(att.name)} — not sent: ${reason}]`;
}

/** Render a user message's text and attachments into a `content` payload. */
export function partsForContent(
  text: string,
  attachments: Attachment[],
): MessageContent {
  if (attachments.length === 0) {
    return text;
  }

  const parts: ContentPart[] = [];
  const trimmedText = text.trim();
  if (trimmedText.length > 0) {
    parts.push({ type: "text", text: trimmedText });
  }

  for (const att of attachments) {
    if (isImageDataUrl(att.data_url)) {
      // The payload decides, not `att.kind`: a screenshot whose extension was
      // not recognised (or a mis-classified binary) is still an image, and a
      // `file` part is exactly what a text-and-image gateway refuses.
      parts.push({ type: "image_url", image_url: { url: att.data_url! } });
    } else if (att.kind === "file" && att.data_url) {
      // Binary attachments have no sendable form. Naming the file keeps it in
      // the model's view without risking the whole turn on a `file` part.
      parts.push({
        type: "text",
        text: unsentFileNote(att, "binary attachments cannot be sent"),
      });
    } else if (att.kind === "audio" && att.data_url) {
      parts.push({
        type: "input_audio",
        input_audio: {
          data: dataUrlBase64(att.data_url),
          // The schema only accepts `wav` and `mp3` (enforced by the picker).
          format: extensionOf(att.name) === "wav" ? "wav" : "mp3",
        },
      });
    } else if (att.kind === "text" && att.text_content !== undefined) {
      parts.push({
        type: "text",
        // The summary says the bytes are already here: a path the model can
        // open is an invitation to read the file a second time, which is one
        // wasted tool round per attachment.
        text: `\n<details><summary>Attached File: ${mentionDisplayName(att.name)} — already provided below</summary>\n\n\`\`\`${fenceLanguage(att)}\n${att.text_content}\n\`\`\`\n</details>`,
      });
    } else if (att.data_url) {
      // Bytes no branch could carry (for example an image-shaped `kind` whose
      // payload is not actually an image). Name it in the transcript instead of
      // dropping it silently — a missing attachment the model never hears about
      // sends the user chasing a bug we already know about.
      parts.push({
        type: "text",
        text: unsentFileNote(att, "its payload could not be attached"),
      });
    }
  }

  return parts;
}
