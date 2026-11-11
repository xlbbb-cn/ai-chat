/**
 * Slot tokens: an invisible marker left in rendered markdown for something that
 * cannot be a React node.
 *
 * The transcript renders replies with markdown-it and `html: false`, so a
 * drawing cannot be markup in the reply text. It is cut out *before* rendering
 * and replaced by a token (`figures.ts` for `svg` / `carve`, `mermaidDiagrams.ts`
 * for Mermaid), and the matching mounter swaps each token for a real element
 * once `innerHTML` is committed. Both mounters had the same DOM walk, so it
 * lives here.
 */

/** U+2063 (INVISIBLE SEPARATOR) wraps the index: markdown-it passes it through
 * untouched, it survives the `breaks: true` line splitting, and it is
 * effectively impossible to collide with real prose. */
export const SLOT_CLOSE = "\u2063";

export const slotPattern = (marker: string): RegExp =>
  new RegExp(`${marker}(\\d+)${SLOT_CLOSE}`, "g");

export const slotToken = (marker: string, index: number): string =>
  `${marker}${index}${SLOT_CLOSE}`;

export interface SlotOptions {
  /** Token prefix, e.g. `\u2063F`. */
  marker: string;
  /** Class on the placeholder `<span>` (hidden by the mounter's stylesheet). */
  className: string;
  /** `data-` attribute holding the index, e.g. `figureSlot`. */
  datasetKey: string;
}

/** Replace every text node holding a slot token with one placeholder per token. */
export function expandSlots(root: HTMLElement, options: SlotOptions): HTMLSpanElement[] {
  const pattern = slotPattern(options.marker);
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const texts: Text[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    // Code and formulas keep their text verbatim: the extractor never puts a
    // slot token there, so one that shows up is the model echoing the marker
    // itself, not a drawing to mount.
    if (node.parentElement?.closest("pre, code, .katex")) continue;
    if (node.data.includes(options.marker)) texts.push(node);
  }
  const slots: HTMLSpanElement[] = [];
  for (const node of texts) {
    const fragment = document.createDocumentFragment();
    for (const part of node.data.split(new RegExp(`(${options.marker}\\d+${SLOT_CLOSE})`, "g"))) {
      pattern.lastIndex = 0;
      const match = pattern.exec(part);
      if (match && part === match[0]) {
        const slot = document.createElement("span");
        slot.className = options.className;
        slot.dataset[options.datasetKey] = match[1];
        fragment.appendChild(slot);
        slots.push(slot);
      } else if (part) {
        fragment.appendChild(document.createTextNode(part));
      }
    }
    node.parentNode?.replaceChild(fragment, node);
  }
  return slots;
}

/** A drawing alone in its paragraph is a block; next to prose it stays inline. */
export function slotHost(slot: HTMLElement): HTMLElement {
  const parent = slot.parentElement;
  if (parent && parent.tagName === "P") {
    const alone = Array.from(parent.childNodes).every(
      (node) => node === slot || (node.nodeType === Node.TEXT_NODE && !(node as Text).data.trim()),
    );
    if (alone) return parent;
  }
  return slot;
}

/** FNV-1a — cheap, and only used to tell one version of a drawing from another. */
export function hashSource(value: string): string {
  let result = 0x811c9dc5;
  for (let i = 0; i < value.length; i += 1) {
    result ^= value.charCodeAt(i);
    result = Math.imul(result, 0x01000193);
  }
  return (result >>> 0).toString(36);
}
