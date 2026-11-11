/**
 * Inline sub-agent picker for the composer (`+agent`).
 *
 * Typing `+` at the start of a line in a **new** conversation opens this popup
 * above the input row; it lists enabled `llm` sub-agents ranked by name and
 * description. Selecting one inserts `+<token>`; on the first send the backend
 * snapshots that agent's system prompt and appends it to the main system prompt
 * for the whole session (see `src-tauri/src/llm_complete.rs`).
 *
 * The popup is not a dialog: it never takes focus (rows accept on `mousedown`
 * with `preventDefault`), so the caret stays in the textarea and the keyboard
 * handling in `App.tsx` keeps working. It reuses the `@file` popup surface
 * (`file-mention-*` in `FileMentionPopup.css` + the two deltas in
 * `AgentMentionPopup.css`) instead of declaring a second popup style.
 */
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faRobot } from "../icons";
import { useI18n } from "../i18n";
import type { AgentMentionOption } from "../agentMention";
import "./FileMentionPopup.css";
import "./AgentMentionPopup.css";

interface AgentMentionPopupProps {
  open: boolean;
  /** Already ranked by the caller and capped to the popup height. */
  items: AgentMentionOption[];
  activeIndex: number;
  loading: boolean;
  /** What the user typed after `+`, for the empty state. */
  query: string;
  onHover: (index: number) => void;
  onSelect: (item: AgentMentionOption) => void;
}

export function AgentMentionPopup({
  open,
  items,
  activeIndex,
  loading,
  query,
  onHover,
  onSelect,
}: AgentMentionPopupProps) {
  const { t } = useI18n();
  if (!open) return null;

  return (
    <div className="file-mention-popup" role="listbox" aria-label={t("app.agentMention.title")}>
      <div className="file-mention-header">{t("app.agentMention.hint")}</div>
      {loading && items.length === 0 ? (
        <div className="file-mention-empty">{t("app.agentMention.loading")}</div>
      ) : items.length === 0 ? (
        <div className="file-mention-empty">
          {query
            ? t("app.agentMention.noMatches", { query })
            : t("app.agentMention.noSelectable")}
        </div>
      ) : (
        items.map((item, index) => (
          <button
            key={item.id}
            type="button"
            role="option"
            aria-selected={index === activeIndex}
            className={`file-mention-item agent-mention-item${index === activeIndex ? " active" : ""}`}
            title={item.description || `+${item.token}`}
            onMouseEnter={() => onHover(index)}
            // mousedown, not click: the textarea must keep the focus (and the
            // caret) for the inserted token to land in the right place.
            onMouseDown={(event) => {
              event.preventDefault();
              onSelect(item);
            }}
          >
            {item.emoji ? (
              // The emoji is authored content from the agent definition (the
              // Sub Agents panel shows it too), not a UI glyph.
              <span className="file-mention-icon agent-mention-emoji" aria-hidden="true">
                {item.emoji}
              </span>
            ) : (
              <span className="file-mention-icon agent-mention-icon" aria-hidden="true">
                <FontAwesomeIcon icon={faRobot} />
              </span>
            )}
            <span className="file-mention-name">{item.name || item.token}</span>
            {item.description && (
              <span className="file-mention-path">{item.description}</span>
            )}
          </button>
        ))
      )}
    </div>
  );
}
