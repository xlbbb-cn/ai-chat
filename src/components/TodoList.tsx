import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getActiveTodoList } from "../api";
import type { TodoListSummary, TodoRecord, TodoStateEvent } from "../types";
import {
    FontAwesomeIcon,
    faChevronRight,
    faCircle,
    faCircleCheck,
    faCircleHalfStroke,
    faCircleXmark,
    faListCheck,
} from "../icons";
import "./TodoList.css";

interface TodoListProps {
    /** Session whose active list this panel mirrors. */
    sessionId?: string;
    /**
     * Only the newest assistant message mirrors the session's list. The panel is
     * mounted once per message, so mirroring in every message would draw the
     * same list again under each historic reply.
     */
    active?: boolean;
}

function statusIcon(status: string) {
    switch (status) {
        case "pending":
            return <FontAwesomeIcon icon={faCircle} />;
        case "in_progress":
            return <FontAwesomeIcon icon={faCircleHalfStroke} />;
        case "completed":
            return <FontAwesomeIcon icon={faCircleCheck} />;
        case "cancelled":
            return <FontAwesomeIcon icon={faCircleXmark} />;
        default:
            return <FontAwesomeIcon icon={faCircle} />;
    }
}

function statusClass(status: string): string {
    switch (status) {
        case "pending":
            return "todo-pending";
        case "in_progress":
            return "todo-in-progress";
        case "completed":
            return "todo-completed";
        case "cancelled":
            return "todo-cancelled";
        default:
            return "todo-pending";
    }
}

function withTodos(list: TodoListSummary, todos: TodoRecord[]): TodoListSummary {
    const count = (status: string) => todos.filter(t => t.status === status).length;
    return {
        ...list,
        todos,
        total: todos.length,
        pending: count("pending"),
        in_progress: count("in_progress"),
        completed: count("completed"),
        cancelled: count("cancelled"),
    };
}

function applyTodoUpdate(
    prev: TodoListSummary | null,
    incoming: TodoRecord
): TodoListSummary | null {
    // Without a snapshot we cannot merge a single record — wait for the next
    // full state. Ignore updates belonging to another (older/newer) list.
    if (!prev) return prev;
    if (prev.list_id !== incoming.list_id) return prev;
    // Append when the item was never seen: these panels mount per message, so
    // they can miss the `todo_state` that introduced a parallel agent's item.
    const known = prev.todos.some(t => t.id === incoming.id);
    const todos = known
        ? prev.todos.map(t => (t.id === incoming.id ? incoming : t))
        : [...prev.todos, incoming];
    return withTodos(prev, todos);
}

function dropCompleted(prev: TodoListSummary): TodoListSummary {
    return withTodos(prev, prev.todos.filter(t => t.status !== "completed"));
}

/** Session an event belongs to, when its payload carries one. */
function eventSessionId(event: TodoStateEvent): string | undefined {
    return event.summary?.session_id ?? event.todo?.session_id ?? event.session_id;
}

export function TodoList({ sessionId, active = false }: TodoListProps) {
    const [todoList, setTodoList] = useState<TodoListSummary | null>(null);
    const [open, setOpen] = useState(false);
    // Bumped on every event: an in-flight initial fetch must not overwrite a
    // snapshot that arrived while it was running.
    const eventSeq = useRef(0);

    const enabled = active && Boolean(sessionId);

    // Mount fetches the session's active list. The panel belongs to one
    // assistant message and has no state of its own, so without this a plan
    // created in an earlier turn (or before a reload) would stay invisible
    // until the model happened to touch the list again.
    useEffect(() => {
        if (!sessionId || !active) {
            setTodoList(null);
            return;
        }
        const seq = eventSeq.current;
        let cancelled = false;
        getActiveTodoList(sessionId)
            .then((list) => {
                if (cancelled || eventSeq.current !== seq) return;
                // A fully completed list stays visible (100% panel); only an
                // absent list hides the panel.
                setTodoList(list ?? null);
            })
            .catch(() => {
                // No active list, or the workspace is switching — stay hidden.
            });
        return () => {
            cancelled = true;
        };
    }, [sessionId, active]);

    useEffect(() => {
        if (!sessionId || !active) return;
        // Subscribe exactly once per (panel, session): re-registering per state
        // change opens a window where events are dropped (unlisten resolves
        // asynchronously). All handlers use functional updates, so no stale
        // closure is read.
        const unlisten = listen<TodoStateEvent>("todo-state", (e) => {
            const event = e.payload;

            // Parallel sessions share this broadcast: a panel must never mirror
            // another session's list.
            const owner = eventSessionId(event);
            if (owner && owner !== sessionId) return;
            eventSeq.current += 1;

            switch (event.type) {
                case "todo_state":
                    // Full snapshot — authoritative, may be null.
                    if (event.summary) {
                        setTodoList(event.summary);
                    }
                    break;

                case "todo_updated":
                    if (event.todo) {
                        const incoming = event.todo;
                        setTodoList(prev => applyTodoUpdate(prev, incoming));
                    }
                    break;

                case "todo_cleared":
                    // Prefer the snapshot shipped with the event; fall back to
                    // dropping completed items from the current list.
                    setTodoList(prev => {
                        if (event.summary) return event.summary;
                        if (!prev || prev.list_id !== event.list_id) return prev;
                        return dropCompleted(prev);
                    });
                    break;

                case "todo_list_changed":
                    // The active list was rotated (summary = the fresh list).
                    setTodoList(event.summary ?? null);
                    break;
            }
        });

        return () => {
            unlisten.then(fn => fn());
        };
    }, [sessionId, active]);

    if (!enabled || !todoList || todoList.todos.length === 0) {
        return null;
    }

    // Nothing left to track: the panel exists to show work in progress, so a
    // finished plan disappears instead of lingering at 100% — the same rule the
    // prompt block follows. "Finished" means nothing pending and nothing
    // running, which also covers a plan whose leftovers were all cancelled.
    if (todoList.pending === 0 && todoList.in_progress === 0) {
        return null;
    }

    const sortedTodos = [...todoList.todos].sort((a, b) => a.position - b.position);
    const activeTodo = sortedTodos.find((t) => t.status === "in_progress");
    // Collapsed preview line: what is being worked on right now, or — when
    // every item is still pending — the next item up, so the second line never
    // goes empty while work remains.
    const previewTodo =
        activeTodo ?? sortedTodos.find((t) => t.status === "pending") ?? null;

    return (
        <div className="todo-list-container">
            <details
                className="todo-list-details"
                open={open}
                onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}
            >
                <summary
                    className={`todo-list-summary${activeTodo ? " todo-list-summary--active" : ""}`}
                >
                    <span className="todo-list-head">
                        <span className="todo-list-icon">
                            <FontAwesomeIcon icon={faListCheck} />
                        </span>
                        <span className="todo-list-title">{todoList.title}</span>
                        <span className="todo-list-stats">
                            {todoList.completed}/{todoList.total}
                        </span>
                        <span className="todo-list-chevron">
                            <FontAwesomeIcon icon={faChevronRight} />
                        </span>
                    </span>
                    {!open && previewTodo && (
                        <span className="todo-list-preview" aria-hidden="true">
                            <span className="todo-preview-row todo-preview-main">
                                <span className="todo-preview-icon">{statusIcon(previewTodo.status)}</span>
                                <span className="todo-preview-title">{previewTodo.title}</span>
                            </span>
                        </span>
                    )}
                </summary>
                <ul className="todo-list-items">
                    {sortedTodos.map((todo) => (
                        <li key={todo.id} className={`todo-item ${statusClass(todo.status)}`}>
                            <span className="todo-item-icon">{statusIcon(todo.status)}</span>
                            <div className="todo-item-content">
                                <div className="todo-item-title">{todo.title}</div>
                                {todo.description && (
                                    <div className="todo-item-description">{todo.description}</div>
                                )}
                            </div>
                        </li>
                    ))}
                </ul>
            </details>
        </div>
    );
}
