/**
 * "Export to PDF", shared by the transcript and the Markdown editor.
 *
 * Both export rendered HTML, so both take the same route: the user names the
 * file once, and Rust prints the document with a headless Chromium — see
 * `src-tauri/src/html_pdf.rs`. The system print dialog is not involved unless
 * the machine has no Edge or Chrome, where `NO_PDF_ENGINE` comes back and the
 * document is printed through the dialog instead: the export degrades rather
 * than failing outright.
 */
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { exportPdf } from "../api";
import { buildPrintDocumentHtml, printHtmlDocument } from "./printDocument";

export interface PdfExportOptions {
  /** Document title, shown by the PDF viewer. */
  title: string;
  /** The rendered markup — a cloned reply, the editor's preview. */
  bodyHtml: string;
  /** Rules only this caller needs, on top of the shared print shell. */
  extraCss?: string;
  /** Filename the save dialog starts with. */
  defaultPath: string;
}

export type PdfExportResult = "saved" | "printed" | "cancelled";

/**
 * Save `bodyHtml` as a PDF, or print it when no browser is installed.
 *
 * Throws when the export itself fails; the caller owns the notice, because only
 * it knows the wording and where the message belongs.
 */
export async function exportHtmlAsPdf(options: PdfExportOptions): Promise<PdfExportResult> {
  const target = await saveDialog({
    defaultPath: options.defaultPath,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  if (!target) return "cancelled";

  const document = {
    title: options.title,
    bodyHtml: options.bodyHtml,
    extraCss: options.extraCss,
  };
  try {
    const saved = await exportPdf(await buildPrintDocumentHtml({ ...document, standalone: true }), target);
    // The file manager is the confirmation: a notice is reserved for warnings
    // and errors.
    void revealItemInDir(saved).catch((error) => console.error("Reveal failed", error));
    return "saved";
  } catch (error) {
    if (String(error) !== "NO_PDF_ENGINE") throw error;
  }
  printHtmlDocument(await buildPrintDocumentHtml(document));
  return "printed";
}
