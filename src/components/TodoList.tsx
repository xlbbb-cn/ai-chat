import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
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

interface TodoRecord {
    id: string;
    session_id: string;
    list_id: string;
    title: string;
    description: string;
    status: string;
    position: number;
    created_at: string;
    updated_at: string;
    completed_at: string | null;
}

interface TodoListSummary {
    list_id: string;
    session_id: string;
    title: string;
    description: string;
    total: number;
    pending: number;
    in_progress: number;
    completed: number;
    cancelled: number;
    created_at: string;
    updated_at: string;
    todos: TodoRecord[];
}

interface TodoStateEvent {
    type: "todo_state" | "todo_updated" | "todo_cleared" | "todo_list_changed";
    list_id?: string;
    session_id?: string;
    summary?: TodoListSummary;
    todo?: TodoRecord;
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

export function TodoList() {
    const [todoList, setTodoList] = useState<TodoListSummary | null>(null);
    const [open, setOpen] = useState(false);

    useEffect(() => {
        // Subscribe exactly once: re-registering per state change opens a
        // window where events are dropped (unlisten resolves asynchronously).
        // All handlers use functional updates, so no stale closure is read.
        const unlisten = listen<TodoStateEvent>("todo-state", (e) => {
            const event = e.payload;

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
    }, []);

    if (!todoList || todoList.todos.length === 0) {
        return null;
    }

    const sortedTodos = [...todoList.todos].sort((a, b) => a.position - b.position);
    const activeTodo = sortedTodos.find((t) => t.status === "in_progress");

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
                    {!open && activeTodo && (
                        <span className="todo-list-preview" aria-hidden="true">
                            <span className="todo-preview-row todo-preview-main">
                                <span className="todo-preview-icon">{statusIcon(activeTodo.status)}</span>
                                <span className="todo-preview-title">{activeTodo.title}</span>
                            </span>
                            <span className="todo-preview-row todo-preview-sub">
                                {activeTodo.description || "\u00A0"}
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
