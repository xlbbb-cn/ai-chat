/**
 * Inline file picker for the composer.
 *
 * Typing `@` in the message box opens this popup above the input row; it lists
 * workspace files filtered by the current model's supported attachment kinds
 * (text is always allowed, images need vision, documents need `supports_files`)
 * and ranked by file name first, path second. Selecting a file inserts
 * `@<path>`; appending `#12`, `#12-24` or `#12-` attaches only those lines.
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
}: FileMentionPopupProps) {
  const { t } = useI18n();
  if (!open) return null;

  return (
    <div className="file-mention-popup" role="listbox" aria-label={t("app.fileMention.title")}>
      <div className="file-mention-header">{t("app.fileMention.hint")}</div>
      {loading && items.length === 0 ? (
        <div className="file-mention-empty">{t("app.fileMention.loading")}</div>
      ) : items.length === 0 ? (
        <div className="file-mention-empty">{t("app.fileMention.noMatches", { query })}</div>
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
              <span className={`file-mention-icon file-mention-kind-${kind}`} aria-hidden="true">
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