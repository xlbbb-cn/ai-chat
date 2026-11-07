/**
 * Inline skill picker for the composer (Claude-Code style `/` commands).
 *
 * Typing `/` in the message box opens this popup above the input row; it lists
 * the workspace skills ranked by name and description. Selecting one inserts
 * `/<skill-name>`; the token stays in the body and the model resolves it with
 * `use_skill`.
 *
 * The popup is not a dialog: it never takes focus (rows accept on `mousedown`
 * with `preventDefault`), so the caret stays in the textarea and the keyboard
 * handling in `App.tsx` keeps working. It deliberately reuses the `@file`
 * popup surface (`file-mention-*` in `FileMentionPopup.css` + the two deltas in
 * `SlashMentionPopup.css`) instead of declaring a second popup style.
 */
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faWandMagicSparkles } from "../icons";
import { useI18n } from "../i18n";
import type { SkillMentionOption } from "../slashMention";
import "./FileMentionPopup.css";
import "./SlashMentionPopup.css";

interface SlashMentionPopupProps {
  open: boolean;
  /** Already ranked by the caller and capped to the popup height. */
  items: SkillMentionOption[];
  activeIndex: number;
  loading: boolean;
  /** What the user typed after `/`, for the empty state. */
  query: string;
  onHover: (index: number) => void;
  onSelect: (item: SkillMentionOption) => void;
}

export function SlashMentionPopup({
  open,
  items,
  activeIndex,
  loading,
  query,
  onHover,
  onSelect,
}: SlashMentionPopupProps) {
  const { t } = useI18n();
  if (!open) return null;

  return (
    <div className="file-mention-popup" role="listbox" aria-label={t("app.slashMention.title")}>
      <div className="file-mention-header">{t("app.slashMention.hint")}</div>
      {loading && items.length === 0 ? (
        <div className="file-mention-empty">{t("app.slashMention.loading")}</div>
      ) : items.length === 0 ? (
        <div className="file-mention-empty">{t("app.slashMention.noMatches", { query })}</div>
      ) : (
        items.map((item, index) => (
          <button
            key={item.name}
            type="button"
            role="option"
            aria-selected={index === activeIndex}
            className={`file-mention-item slash-mention-item${index === activeIndex ? " active" : ""}`}
            title={item.description || `/${item.name}`}
            onMouseEnter={() => onHover(index)}
            // mousedown, not click: the textarea must keep the focus (and the
            // caret) for the inserted token to land in the right place.
            onMouseDown={(event) => {
              event.preventDefault();
              onSelect(item);
            }}
          >
            <span className="file-mention-icon slash-mention-icon" aria-hidden="true">
              <FontAwesomeIcon icon={faWandMagicSparkles} />
            </span>
            <span className="file-mention-name">{item.name}</span>
            {item.description && (
              <span className="file-mention-path">{item.description}</span>
            )}
          </button>
        ))
      )}
    </div>
  );
}
