/**
 * Inline file picker for the composer.
 *
 * Typing `@` in the message box opens this popup above the input row; it lists
 * workspace files filtered by the current model's supported attachment kinds
 * (text is always allowed, images need vision, binaries are not offered)
 * and ranked by file name first, path second. Selecting a file inserts
 * `@<path>`; appending `#12`, `#12-24` or `#12-` attaches only those lines.
 * A PDF is the exception: it is offered to every model because the backend
 * converts the mentioned pages to Markdown text (`convert_pdf_mention`), so
 * `#2` means page 2 there instead of line 2.
 *
 * The same surface serves the `&path` picker (`mode="path"`, see `pathRef.ts`):
 * one list, one keyboard contract and one look — only the header text promises
 * the opposite, that the file's content is *not* sent with the message.
 *
 * The popup is not a dialog: it never takes focus (rows accept on `mousedown`
 * with `preventDefault`), so the caret stays in the textarea and the keyboard
 * handling in `App.tsx` keeps working.
 */
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  faFileLines,
  faFilePen,
  faImage,
  faVolumeHigh,
} from "@fortawesome/free-solid-svg-icons";
import { classifyName } from "../attachments";
import { useI18n } from "../i18n";
import type { WorkspaceFileEntry } from "../types";
import "./FileMentionPopup.css";

interface FileMentionPopupProps {
  open: boolean;
  /** Already filtered (model capabilities) and ranked, capped by the caller. */
  items: WorkspaceFileEntry[];
  activeIndex: number;
  loading: boolean;
  /** What the user typed after `@`, for the empty state. */
  query: string;
  onHover: (index: number) => void;
  onSelect: (item: WorkspaceFileEntry) => void;
  /**
   * Which token the surface serves. `attach` (default) is the `@file` picker:
   * the bytes ride along, so the header promises them and the caller filters
   * the list by what the model can consume. `path` is the `&file` picker
   * (`pathRef.ts`): same list, same surface, same keys — only the header says
   * that the content is *not* sent and that there is no `#` range.
   */
  mode?: "attach" | "path";
}

function iconFor(kind: ReturnType<typeof classifyName>) {
  switch (kind) {
    case "image":
      return faImage;
    case "audio":
      return faVolumeHigh;
    case "file":
      return faFileLines;
    default:
      return faFilePen;
  }
}

function directoryOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash > 0 ? path.slice(0, slash) : "";
}

export function FileMentionPopup({
  open,
  items,
  activeIndex,
  loading,
  query,
  onHover,
  onSelect,
  mode = "attach",
}: FileMentionPopupProps) {
  const { t } = useI18n();
  if (!open) return null;

  const title = mode === "path" ? t("app.pathRef.title") : t("app.fileMention.title");
  const hint = mode === "path" ? t("app.pathRef.hint") : t("app.fileMention.hint");

  return (
    <div className="file-mention-popup" role="listbox" aria-label={title}>
      <div className="file-mention-header">{hint}</div>
      {loading && items.length === 0 ? (
        <div className="file-mention-empty">{t("app.fileMention.loading")}</div>
      ) : items.length === 0 ? (
        <div className="file-mention-empty">
          {mode === "path"
            ? t("app.pathRef.noMatches", { query })
            : t("app.fileMention.noMatches", { query })}
        </div>
      ) : (
        items.map((item, index) => {
          const kind = classifyName(item.name);
          const directory = directoryOf(item.path);
          return (
            <button
              key={item.path}
              type="button"
              role="option"
              aria-selected={index === activeIndex}
              className={`file-mention-item${index === activeIndex ? " active" : ""}`}
              title={item.path}
              onMouseEnter={() => onHover(index)}
              // mousedown, not click: the textarea must keep the focus (and the
              // caret) for the inserted token to land in the right place.
              onMouseDown={(event) => {
                event.preventDefault();
                onSelect(item);
              }}
            >
              {/* The kind colour answers "will the model accept this?" — a
                  question the `&file` picker never asks, so it stays neutral
                  there (the row shape is identical either way). */}
              <span
                className={`file-mention-icon${
                  mode === "attach" ? ` file-mention-kind-${kind}` : ""
                }`}
                aria-hidden="true"
              >
                <FontAwesomeIcon icon={iconFor(kind)} />
              </span>
              <span className="file-mention-name">{item.name}</span>
              {directory && <span className="file-mention-path">{directory}</span>}
            </button>
          );
        })
      )}
    </div>
  );
}
