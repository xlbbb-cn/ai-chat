/**
 * Standalone print / PDF documents.
 *
 * Two callers build a document here, and both need the same shell: a document
 * of its own has none of the app's CSS, so the light palette and the prose
 * rules are spelled out once instead of drifting apart.
 *
 * - The reply's "export to PDF" writes the document into a hidden iframe and
 *   calls `print()`, so the user's printer — or "Save as PDF" — produces the
 *   file.
 * - The Markdown editor's PDF export hands the same HTML to Rust, which prints
 *   it with a headless Chromium into a real `.pdf` file. That document must be
 *   *self-contained*: it is opened from a temp directory by another process,
 *   where the app's asset URLs mean nothing. `standalone: true` therefore
 *   inlines KaTeX (fonts as data URLs) and drops the bundled web font.
 */
import katexCssUrl from "katex/dist/katex.min.css?url";
import katexCssText from "katex/dist/katex.min.css?raw";
import razerF5FontUrl from "../assets/Web Fonts/9b77a61b70b873a0f298ad6fbc801666.woff2?url";

/** Font stack of a standalone document: system faces, nothing downloaded. */
const SYSTEM_FONT_STACK =
  '"Segoe UI", "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", "Helvetica Neue", Arial, sans-serif';

const CODE_FONT_STACK = '"Cascadia Code", "Consolas", "Microsoft YaHei", monospace';

export const escapeHtml = (value: string): string =>
  value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\"/g, "&quot;")
    .replace(/'/g, "&#39;");

/**
 * Every font `katex.min.css` points at.
 *
 * The CSS is imported as text (`?raw`) so its `url(fonts/…)` targets can be
 * swapped for data URLs; the matching files are collected here as asset URLs
 * and fetched on demand. Vite emits them as separate files, so the bundle only
 * grows by the URLs, not by the fonts.
 */
const katexFontUrls = import.meta.glob<string>("/node_modules/katex/dist/fonts/*.woff2", {
  eager: true,
  query: "?url",
  import: "default",
});

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  // `String.fromCharCode(...bytes)` blows the stack on a 40 kB font.
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

let katexInlineCss: Promise<string> | null = null;

/** `katex.min.css` with every woff2 font embedded — resolved once per session. */
function inlineKatexCss(): Promise<string> {
  katexInlineCss ??= (async () => {
    const encoded = new Map<string, string>();
    await Promise.all(
      Object.entries(katexFontUrls).map(async ([path, url]) => {
        const name = path.slice(path.lastIndexOf("/") + 1);
        try {
          const response = await fetch(url);
          encoded.set(name, toBase64(new Uint8Array(await response.arrayBuffer())));
        } catch (error) {
          // A missing face costs typography, not the export.
          console.error(`KaTeX font ${name} could not be embedded`, error);
        }
      }),
    );
    return katexCssText.replace(
      /url\((["']?)fonts\/([^)"']+)\1\)/g,
      (match, _quote: string, file: string) => {
        if (!file.endsWith(".woff2")) return match;
        const data = encoded.get(file);
        return data ? `url("data:font/woff2;base64,${data}")` : match;
      },
    );
  })();
  return katexInlineCss;
}

/**
 * The palette a document of its own needs.
 *
 * A figure SVG is written in theme tokens, and a printed page is always light,
 * so the light values are pinned here instead of read from the app.
 */
function baseCss(fontFamily: string): string {
  return `
      :root {
        color-scheme: light;
        font-family: ${fontFamily};
        --c-surface: #ffffff;
        --c-surface-2: #f6f7f8;
        --c-bg: #ffffff;
        --c-text: #171a20;
        --c-body: #2b2f36;
        --c-muted: #5c5e62;
        --c-border-s: #e8eaed;
        --c-border-i: #d9dce1;
        --c-accent: #3e6ae1;
        --c-success: #1f8a3b;
        --c-warning: #b07500;
        --c-danger: #c0392b;
      }

      * {
        box-sizing: border-box;
      }

      /* A page box, not a screen: the sheet supplies the margins, so the body
         carries none. */
      @page {
        size: A4;
        margin: 16mm;
      }

      body {
        margin: 0;
        padding: 0;
        background: #ffffff;
        color: #171a20;
        font-size: 14px;
        line-height: 1.6;
        word-break: break-word;
      }

      main {
        width: 100%;
      }

      p {
        margin: 0 0 0.75em;
      }

      ul,
      ol {
        margin: 0 0 0.75em;
        padding-left: 1.5em;
      }

      li {
        margin-bottom: 0.25em;
      }

      h1,
      h2,
      h3,
      h4,
      h5,
      h6 {
        color: #171a20;
        break-after: avoid;
      }

      pre {
        margin: 0.75em 0;
        padding: 12px 14px;
        background: #f4f4f4;
        border: 1px solid #e5e7eb;
        border-radius: 8px;
        overflow-x: auto;
        white-space: pre-wrap;
        break-inside: avoid;
      }

      code {
        font-family: ${CODE_FONT_STACK};
      }

      table {
        display: table;
        width: 100%;
        border-collapse: collapse;
        margin: 0.75em 0;
      }

      /* Rows break, tables do not: a long table runs over pages. */
      tr {
        break-inside: avoid;
      }

      th,
      td {
        border: 1px solid #d1d5db;
        padding: 8px 10px;
        text-align: left;
        vertical-align: top;
      }

      th {
        background: #f3f4f6;
      }

      blockquote {
        margin: 0.75em 0;
        padding: 4px 12px;
        border-left: 3px solid #d9dce1;
        color: #5c5e62;
        background: #f6f7f8;
        break-inside: avoid;
      }

      hr {
        border: none;
        border-top: 1px solid #e8eaed;
        margin: 1em 0;
      }

      img,
      svg,
      video {
        max-width: 100%;
        height: auto;
        break-inside: avoid;
      }
`;
}

export interface PrintDocumentOptions {
  /** Document title — the filename a "Save as PDF" dialog suggests. */
  title: string;
  /** Already-rendered markup for the `<main>` element. */
  bodyHtml: string;
  /** Rules only this caller needs, appended after the shared ones. */
  extraCss?: string;
  /** Inline KaTeX and use system fonts — required outside the app's webview. */
  standalone?: boolean;
}

/** A complete HTML document for printing or for the headless PDF engine. */
export async function buildPrintDocumentHtml(options: PrintDocumentOptions): Promise<string> {
  const { title, bodyHtml, extraCss = "", standalone = false } = options;
  const fontFamily = standalone ? SYSTEM_FONT_STACK : `"RazerF5", ${SYSTEM_FONT_STACK}`;
  const fontFace = standalone
    ? ""
    : `@font-face {
        font-family: "RazerF5";
        src: url("${razerF5FontUrl}") format("woff2");
        font-weight: 400;
        font-style: normal;
      }`;
  const katex = standalone
    ? `<style>${await inlineKatexCss()}</style>`
    : `<link rel="stylesheet" href="${escapeHtml(katexCssUrl)}" data-katex-css />`;

  return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>${escapeHtml(title)}</title>
    ${katex}
    <style>
      ${fontFace}
      ${baseCss(fontFamily)}
      ${extraCss}
    </style>
  </head>
  <body>
    <main>${bodyHtml}</main>
  </body>
</html>`;
}

/**
 * Print a document through the system print dialog.
 *
 * The frame is a print surface, not a preview: it is zero-sized, never
 * focusable, and removed once the dialog closes.
 */
export function printHtmlDocument(html: string): void {
  const iframe = document.createElement("iframe");

  iframe.style.position = "fixed";
  iframe.style.right = "0";
  iframe.style.bottom = "0";
  iframe.style.width = "0";
  iframe.style.height = "0";
  iframe.style.border = "0";
  iframe.setAttribute("aria-hidden", "true");

  const cleanup = () => {
    iframe.remove();
  };

  iframe.addEventListener("load", () => {
    const frameWindow = iframe.contentWindow;
    const frameDocument = frameWindow?.document;
    if (!frameWindow || !frameDocument) {
      cleanup();
      return;
    }

    frameDocument.open();
    frameDocument.write(html);
    frameDocument.close();

    const handleAfterPrint = () => {
      frameWindow.removeEventListener("afterprint", handleAfterPrint);
      window.setTimeout(cleanup, 0);
    };

    frameWindow.addEventListener("afterprint", handleAfterPrint);

    // Lay out the formulas before printing, whatever carries their stylesheet.
    let printed = false;
    const doPrint = () => {
      if (printed) return;
      printed = true;
      frameWindow.focus();
      frameWindow.print();
    };

    const katexLink = frameDocument.querySelector<HTMLLinkElement>("link[data-katex-css]");
    if (katexLink) {
      katexLink.addEventListener("load", doPrint, { once: true });
      katexLink.addEventListener("error", doPrint, { once: true });
      // Fallback in case neither event fires (e.g. served from cache).
      window.setTimeout(doPrint, 400);
    } else {
      frameDocument.fonts.ready.then(doPrint, doPrint);
      window.setTimeout(doPrint, 400);
    }
  }, { once: true });

  document.body.appendChild(iframe);
}

/**
 * Page rules for a Markdown document.
 *
 * Mirrors `MarkdownPreview.css` with the light values spelled out, because the
 * preview's own stylesheet is not part of the printed document. The preview
 * styles a table as a scrollable block — on paper that clips it, so the table
 * is a real table here.
 */
export const MARKDOWN_PRINT_CSS = `
      .md-preview {
        padding-left: 0;
        font-size: 13px;
        line-height: 1.65;
      }

      .md-preview ul,
      .md-preview ol {
        padding-left: 1.2em;
        margin-left: 0;
      }

      .md-preview p {
        margin: 0 0 0.6em;
      }

      .md-preview h1 {
        font-size: 1.6em;
      }

      .md-preview h2 {
        font-size: 1.35em;
      }

      .md-preview h3 {
        font-size: 1.15em;
      }

      .md-preview h4,
      .md-preview h5,
      .md-preview h6 {
        font-size: 1em;
      }

      .md-preview a {
        color: #3e6ae1;
        text-decoration: none;
      }

      .md-preview hr {
        border: none;
        border-top: 0.5px solid #e8eaed;
        margin: 1em 0;
      }

      .md-preview blockquote {
        margin: 0.6em 0;
        padding: 4px 12px;
        border-left: 3px solid #d9dce1;
        color: #5c5e62;
        background: #f6f7f8;
        border-radius: 0 4px 4px 0;
        break-inside: avoid;
      }

      .md-preview code {
        background: #f4f4f4;
        color: #171a20;
        border-radius: 4px;
        padding: 1px 5px;
        font-size: 0.92em;
      }

      .md-preview pre {
        background: #f4f4f4;
        color: #171a20;
        border: 0.5px solid #d9dce1;
        border-radius: 6px;
        padding: 10px 12px;
        font-size: 12px;
        line-height: 1.55;
        overflow: visible;
        white-space: pre-wrap;
        break-inside: avoid;
      }

      .md-preview pre code {
        background: none;
        padding: 0;
      }

      .md-preview table {
        display: table;
        width: 100%;
        font-size: 12px;
        overflow: visible;
      }

      .md-preview th,
      .md-preview td {
        border: 0.5px solid #d9dce1;
        padding: 7px 10px;
        text-align: left;
        vertical-align: top;
      }

      .md-preview thead th {
        background: #f3f4f6;
        color: #171a20;
      }

      .md-preview .katex-display {
        overflow: visible;
        margin: 0.6em 0;
        break-inside: avoid;
      }
`;
