import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
    archiveTodoList,
    clearCompletedTodos,
    createSessionTodo,
    createTodoList,
    deleteTodoCmd,
    getSessionTodo,
    updateTodoStatusCmd,
    updateTodoTextCmd,
} from "../api";
import type { TodoListSummary, TodoRecord, TodoStatus } from "../types";
import "./TodoPanel.css";

interface Props {
    sessionId: string;
    onClose: () => void;
}

type StatusFilter = "active" | "all" | "completed";

const STATUS_OPTIONS: { value: TodoStatus; label: string }[] = [
    { value: "pending", label: "Pending" },
    { value: "in_progress", label: "In progress" },
    { value: "completed", label: "Completed" },
    { value: "cancelled", label: "Cancelled" },
];

function isTodoStatus(value: string): value is TodoStatus {
    return (
        value === "pending" ||
        value === "in_progress" ||
        value === "completed" ||
        value === "cancelled"
    );
}

function shortId(id: string): string {
    return id.length >= 8 ? id.slice(0, 8) : id;
}

export function TodoPanel({ sessionId, onClose }: Props) {
    const [list, setList] = useState<TodoListSummary | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [draftTitle, setDraftTitle] = useState("");
    const [draftDescription, setDraftDescription] = useState("");
    const [filter, setFilter] = useState<StatusFilter>("active");
    const [savingId, setSavingId] = useState<string | null>(null);

    const refresh = useCallback(async () => {
        try {
            setLoading(true);
            setError(null);
            const data = await getSessionTodo(sessionId);
            setList(data);
        } catch (err) {
            console.error("Failed to load todo list:", err);
            setError(String(err));
        } finally {
            setLoading(false);
        }
    }, [sessionId]);

    useEffect(() => {
        refresh();
    }, [refresh]);

    useEffect(() => {
        const promises: Promise<() => void>[] = [];
        promises.push(
            listen("todo-state", () => {
                // Any todo mutation triggers a refresh.
                refresh();
            }),
        );
        return () => {
            promises.forEach((p) => p.then((fn) => fn()).catch(() => undefined));
        };
    }, [refresh]);

    async function handleAdd() {
        const title = draftTitle.trim();
        if (!title) return;
        const description = draftDescription.trim();
        try {
            setLoading(true);
            setError(null);
            const updated = await createSessionTodo(sessionId, title, description);
            setList(updated);
            setDraftTitle("");
            setDraftDescription("");
        } catch (err) {
            setError(String(err));
        } finally {
            setLoading(false);
        }
    }

    async function handleStatus(todo: TodoRecord, next: TodoStatus) {
        if (todo.status === next) return;
        try {
            setSavingId(todo.id);
            const updated = await updateTodoStatusCmd(todo.id, next);
            setList((prev) => (prev ? mergeRecord(prev, updated) : prev));
        } catch (err) {
            setError(String(err));
        } finally {
            setSavingId(null);
        }
    }

    async function handleDelete(todo: TodoRecord) {
        try {
            setSavingId(todo.id);
            await deleteTodoCmd(todo.id);
            setList((prev) =>
                prev
                    ? {
                        ...prev,
                        todos: prev.todos.filter((t) => t.id !== todo.id),
                        total: Math.max(0, prev.total - 1),
                    }
                    : prev
            );
        } catch (err) {
            setError(String(err));
        } finally {
            setSavingId(null);
        }
    }

    async function handleCommitText(
        todo: TodoRecord,
        patch: { title?: string; description?: string }
    ) {
        if (
            (patch.title !== undefined && patch.title === todo.title) ||
            (patch.description !== undefined && patch.description === todo.description)
        ) {
            return;
        }
        try {
            setSavingId(todo.id);
            const updated = await updateTodoTextCmd(todo.id, patch);
            setList((prev) => (prev ? mergeRecord(prev, updated) : prev));
        } catch (err) {
            setError(String(err));
        } finally {
            setSavingId(null);
        }
    }

    async function handleClearCompleted() {
        try {
            setLoading(true);
            const updated = await clearCompletedTodos(sessionId);
            setList(updated);
        } catch (err) {
            setError(String(err));
        } finally {
            setLoading(false);
        }
    }

    async function handleArchive() {
        try {
            setLoading(true);
            const updated = await createTodoList(sessionId, "Working plan", "");
            setList(updated);
        } catch (err) {
            setError(String(err));
        } finally {
            setLoading(false);
        }
    }

    async function handleArchiveOnly() {
        if (!list) return;
        try {
            setLoading(true);
            await archiveTodoList(list.list_id);
            const next = await getSessionTodo(sessionId);
            setList(next);
        } catch (err) {
            setError(String(err));
        } finally {
            setLoading(false);
        }
    }

    const filtered = list
        ? filter === "all"
            ? list.todos
            : filter === "completed"
                ? list.todos.filter((t) => t.status === "completed" || t.status === "cancelled")
                : list.todos.filter((t) => t.status === "pending" || t.status === "in_progress")
        : [];

    return (
        <div className="todo-panel">
            <div className="todo-header">
                <div className="todo-header-text">
                    <h2>{list?.title ?? "Todo List"}</h2>
                    <p>
                        Track complex work step-by-step. The assistant reads and updates this list to stay focused.
                    </p>
                </div>
                <div className="todo-header-actions">
                    <button
                        className="todo-refresh"
                        onClick={refresh}
                        disabled={loading}
                        title="Refresh from server"
                    >
                        ⟳
                    </button>
                    <button className="close-btn" onClick={onClose} title="Close panel">
                        ✕
                    </button>
                </div>
            </div>

            {error && <div className="error-banner">{error}</div>}

            {list && (
                <div className="todo-summary">
                    <strong>{list.total}</strong> total
                    <span className="todo-summary-pill todo-pill-pending">pending {list.pending}</span>
                    <span className="todo-summary-pill todo-pill-in_progress">
                        in progress {list.in_progress}
                    </span>
                    <span className="todo-summary-pill todo-pill-completed">completed {list.completed}</span>
                    {list.cancelled > 0 && (
                        <span className="todo-summary-pill todo-pill-cancelled">
                            cancelled {list.cancelled}
                        </span>
                    )}
                    <span style={{ marginLeft: "auto", display: "flex", gap: 4 }}>
                        {(["active", "all", "completed"] as StatusFilter[]).map((f) => (
                            <button
                                key={f}
                                className={`todo-refresh ${filter === f ? "" : ""}`}
                                onClick={() => setFilter(f)}
                                style={{
                                    background: filter === f ? "var(--c-active)" : undefined,
                                    color: filter === f ? "var(--c-text)" : undefined,
                                }}
                            >
                                {f}
                            </button>
                        ))}
                    </span>
                </div>
            )}

            <div className="todo-list">
                {!list && !loading && (
                    <div className="todo-empty">
                        <strong>No active todo list yet.</strong>
                        Add a todo below — the assistant will be able to read and update it during the
                        conversation.
                    </div>
                )}
                {list && list.todos.length === 0 && !loading && (
                    <div className="todo-empty">
                        <strong>This list is empty.</strong>
                        Add a todo below, or ask the assistant to plan out the work — it will populate this
                        list automatically.
                    </div>
                )}
                {filtered.length === 0 && list && list.todos.length > 0 && (
                    <div className="todo-empty">
                        <strong>No todos match the {filter} filter.</strong>
                    </div>
                )}
                {filtered.map((todo) => (
                    <TodoItem
                        key={todo.id}
                        todo={todo}
                        disabled={savingId === todo.id}
                        onStatus={handleStatus}
                        onDelete={handleDelete}
                        onCommitText={handleCommitText}
                    />
                ))}
            </div>

            <div className="todo-composer">
                <input
                    className="todo-composer-input"
                    placeholder="Add a new todo — e.g. Investigate build failure"
                    value={draftTitle}
                    onChange={(e) => setDraftTitle(e.target.value)}
                    onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.shiftKey) {
                            e.preventDefault();
                            handleAdd();
                        }
                    }}
                />
                <textarea
                    className="todo-composer-textarea"
                    placeholder="Optional description or acceptance criteria"
                    value={draftDescription}
                    onChange={(e) => setDraftDescription(e.target.value)}
                />
                <div className="todo-composer-actions">
                    <span style={{ fontSize: 11, color: "var(--c-muted)" }}>
                        Press Enter to add quickly
                    </span>
                    <div className="todo-composer-buttons">
                        <button
                            className="todo-composer-btn"
                            onClick={handleClearCompleted}
                            disabled={!list || list.completed === 0}
                        >
                            Clear completed
                        </button>
                        <button
                            className="todo-composer-btn todo-composer-btn-primary"
                            onClick={handleAdd}
                            disabled={!draftTitle.trim()}
                        >
                            Add todo
                        </button>
                    </div>
                </div>
            </div>

            {list && list.todos.length > 0 && (
                <div className="todo-archive-row">
                    <span className="todo-archive-hint">
                        Archive the list when a phase of work is done and start a fresh one.
                    </span>
                    <div className="todo-composer-buttons">
                        {list.todos.some((t) => t.status !== "completed" && t.status !== "cancelled") && (
                            <button className="todo-composer-btn" onClick={handleArchiveOnly}>
                                Archive current
                            </button>
                        )}
                        <button className="todo-composer-btn" onClick={handleArchive}>
                            Start new list
                        </button>
                    </div>
                </div>
            )}
        </div>
    );
}

interface TodoItemProps {
    todo: TodoRecord;
    disabled: boolean;
    onStatus: (todo: TodoRecord, next: TodoStatus) => void;
    onDelete: (todo: TodoRecord) => void;
    onCommitText: (
        todo: TodoRecord,
        patch: { title?: string; description?: string }
    ) => void;
}

function TodoItem({ todo, disabled, onStatus, onDelete, onCommitText }: TodoItemProps) {
    const [title, setTitle] = useState(todo.title);
    const [description, setDescription] = useState(todo.description);

    useEffect(() => {
        setTitle(todo.title);
    }, [todo.title]);

    useEffect(() => {
        setDescription(todo.description);
    }, [todo.description]);

    const status = isTodoStatus(todo.status) ? todo.status : "pending";

    const itemClass = [
        "todo-item",
        status === "completed" ? "todo-item-completed" : "",
        status === "cancelled" ? "todo-item-cancelled" : "",
    ]
        .filter(Boolean)
        .join(" ");

    return (
        <div className={itemClass}>
            <div className="todo-item-header">
                <button
                    className={`todo-item-status todo-status-${status}`}
                    title={
                        status === "completed"
                            ? "Completed — click to reopen"
                            : status === "in_progress"
                                ? "Mark as completed"
                                : "Mark as in progress"
                    }
                    disabled={disabled}
                    onClick={() => {
                        if (status === "completed" || status === "cancelled") {
                            onStatus(todo, "pending");
                        } else if (status === "pending") {
                            onStatus(todo, "in_progress");
                        } else {
                            onStatus(todo, "completed");
                        }
                    }}
                >
                    {status === "completed" ? "✓" : status === "in_progress" ? "•" : ""}
                </button>
                <div className="todo-item-body">
                    <input
                        className="todo-item-title"
                        value={title}
                        disabled={disabled}
                        onChange={(e) => setTitle(e.target.value)}
                        onBlur={() => {
                            const trimmed = title.trim();
                            if (trimmed && trimmed !== todo.title) {
                                onCommitText(todo, { title: trimmed });
                            } else {
                                setTitle(todo.title);
                            }
                        }}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") {
                                e.preventDefault();
                                (e.target as HTMLInputElement).blur();
                            }
                        }}
                    />
                    <textarea
                        className="todo-item-description"
                        value={description}
                        disabled={disabled}
                        placeholder="Add a description..."
                        onChange={(e) => setDescription(e.target.value)}
                        onBlur={() => {
                            const trimmed = description.trim();
                            if (trimmed !== todo.description) {
                                onCommitText(todo, { description: trimmed });
                            } else {
                                setDescription(todo.description);
                            }
                        }}
                        rows={2}
                    />
                </div>
            </div>
            <div className="todo-item-footer">
                <div className="todo-item-meta">
                    <span>id: {shortId(todo.id)}</span>
                    <select
                        className="todo-status-select"
                        value={status}
                        disabled={disabled}
                        onChange={(e) => onStatus(todo, e.target.value as TodoStatus)}
                    >
                        {STATUS_OPTIONS.map((opt) => (
                            <option key={opt.value} value={opt.value}>
                                {opt.label}
                            </option>
                        ))}
                    </select>
                </div>
                <button
                    className="todo-delete-btn"
                    title="Delete todo"
                    disabled={disabled}
                    onClick={() => onDelete(todo)}
                >
                    ✕
                </button>
            </div>
        </div>
    );
}

function mergeRecord(list: TodoListSummary, record: TodoRecord): TodoListSummary {
    const todos = list.todos.map((t) => (t.id === record.id ? record : t));
    const counts = countStatuses(todos);
    return {
        ...list,
        todos,
        total: todos.length,
        pending: counts.pending,
        in_progress: counts.in_progress,
        completed: counts.completed,
        cancelled: counts.cancelled,
    };
}

function countStatuses(todos: TodoRecord[]) {
    const counts = { pending: 0, in_progress: 0, completed: 0, cancelled: 0 };
    for (const t of todos) {
        if (t.status === "pending") counts.pending += 1;
        else if (t.status === "in_progress") counts.in_progress += 1;
        else if (t.status === "completed") counts.completed += 1;
        else if (t.status === "cancelled") counts.cancelled += 1;
    }
    return counts;
}
