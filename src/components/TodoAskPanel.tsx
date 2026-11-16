import { useState } from "react";
import { useI18n } from "../i18n";
import type { TodoAskAnswer, TodoAskRequest, TodoAskQuestion } from "../types";
import {
    FontAwesomeIcon,
    faCircle,
    faCircleCheck,
    faListCheck,
} from "../icons";

interface TodoAskPanelProps {
    /** The pending form as announced by the `todo-ask` event. */
    request: TodoAskRequest;
    /** Submit every answer; the backend unblocks the waiting tool call. */
    onSubmit: (answers: TodoAskAnswer[]) => void;
    /** Dismiss the form without answering (the tool reports "dismissed"). */
    onDismiss: () => void;
}

/**
 * The interactive question form of the `todos` tool's `ask` action.
 *
 * One tab per question, one card under the tabs showing that question with its
 * selectable options and/or its manual input, and one submit button in the
 * footer that answers the whole form. The panel mounts only while a form is
 * pending; the backend serializes asks, so at most one is ever open.
 */
export function TodoAskPanel({ request, onSubmit, onDismiss }: TodoAskPanelProps) {
    const { t } = useI18n();
    const [activeIndex, setActiveIndex] = useState(0);
    /** Chosen option per question id (null = not chosen / custom is in use). */
    const [selected, setSelected] = useState<Record<string, string | null>>({});
    /** Typed manual input per question id. */
    const [texts, setTexts] = useState<Record<string, string>>({});

    const questions = request.questions;
    const question = questions[Math.min(activeIndex, Math.max(questions.length - 1, 0))];

    /**
     * The effective answer of one question: typed text wins over a chosen
     * option (typing clears the choice and vice versa, so exactly one of the
     * two is ever set).
     */
    const valueOf = (q: TodoAskQuestion): string => {
        const text = (texts[q.id] ?? "").trim();
        if (text) return text;
        return selected[q.id] ?? "";
    };

    // Derived per render — at most 8 questions, cheaper than memo bookkeeping.
    const values = questions.map((q) => ({ id: q.id, value: valueOf(q) }));
    const answeredCount = values.filter((v) => v.value !== "").length;
    const complete = answeredCount === questions.length && questions.length > 0;

    if (questions.length === 0 || !question) {
        return null;
    }

    const chooseOption = (qid: string, option: string) => {
        setTexts((prev) => ({ ...prev, [qid]: "" }));
        setSelected((prev) => ({ ...prev, [qid]: option }));
    };

    const typeCustom = (qid: string, text: string) => {
        setSelected((prev) => ({ ...prev, [qid]: null }));
        setTexts((prev) => ({ ...prev, [qid]: text }));
    };

    const submit = () => {
        if (!complete) return;
        onSubmit(values.filter((v) => v.value !== ""));
    };

    return (
        <div className="todo-ask-panel" role="region" aria-label={t("app.ask.title")}>
            <div className="todo-ask-header">
                <FontAwesomeIcon icon={faListCheck} className="todo-ask-icon" />
                <span className="todo-ask-title">
                    {request.topic?.trim() || t("app.ask.title")}
                </span>
                <span className="todo-ask-progress">
                    {t("app.ask.progress", {
                        answered: answeredCount,
                        total: questions.length,
                    })}
                </span>
            </div>
            {questions.length > 1 && (
                <div className="todo-ask-tabs" role="tablist">
                    {questions.map((q, i) => {
                        const answered = values[i].value !== "";
                        return (
                            <button
                                key={q.id}
                                type="button"
                                role="tab"
                                aria-selected={i === activeIndex}
                                className={
                                    "todo-ask-tab" +
                                    (i === activeIndex ? " todo-ask-tab--active" : "")
                                }
                                title={q.title}
                                onClick={() => setActiveIndex(i)}
                            >
                                <span className="todo-ask-tab-icon">
                                    <FontAwesomeIcon
                                        icon={answered ? faCircleCheck : faCircle}
                                    />
                                </span>
                                <span className="todo-ask-tab-label">
                                    {t("app.ask.tabN", { n: i + 1 })}
                                </span>
                            </button>
                        );
                    })}
                </div>
            )}
            <div className="todo-ask-body">
                <div className="todo-ask-card" role="tabpanel">
                    <div className="todo-ask-question">{question.title}</div>
                    {question.description?.trim() && (
                        <div className="todo-ask-description">
                            {question.description.trim()}
                        </div>
                    )}
                    {question.options && question.options.length > 0 && (
                        <div className="todo-ask-options" role="radiogroup">
                            {question.options.map((option) => {
                                const chosen =
                                    valueOf(question) === option;
                                return (
                                    <button
                                        key={option}
                                        type="button"
                                        role="radio"
                                        aria-checked={chosen}
                                        className={
                                            "todo-ask-option" +
                                            (chosen ? " todo-ask-option--selected" : "")
                                        }
                                        onClick={() => chooseOption(question.id, option)}
                                    >
                                        <span className="todo-ask-option-icon">
                                            <FontAwesomeIcon
                                                icon={chosen ? faCircleCheck : faCircle}
                                            />
                                        </span>
                                        <span className="todo-ask-option-label">{option}</span>
                                    </button>
                                );
                            })}
                        </div>
                    )}
                    {question.allow_custom && (
                        <textarea
                            className="todo-ask-input"
                            rows={2}
                            value={texts[question.id] ?? ""}
                            placeholder={t("app.ask.customPlaceholder")}
                            onChange={(e) => typeCustom(question.id, e.target.value)}
                        />
                    )}
                </div>
            </div>
            <div className="todo-ask-actions">
                <span className="todo-ask-hint">
                    {complete ? t("app.ask.ready") : t("app.ask.incomplete")}
                </span>
                <button type="button" className="btn-secondary" onClick={onDismiss}>
                    {t("app.ask.dismiss")}
                </button>
                <button
                    type="button"
                    className="btn-primary"
                    disabled={!complete}
                    onClick={submit}
                >
                    {t("app.ask.submit")}
                </button>
            </div>
        </div>
    );
}
