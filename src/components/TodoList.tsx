import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
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

function statusIcon(status: string): string {
    switch (status) {
        case "pending":
            return "○";
        case "in_progress":
            return "◐";
        case "completed":
            return "✓";
        case "cancelled":
            return "✕";
        default:
            return "○";
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

export function TodoList() {
    const [todoList, setTodoList] = useState<TodoListSummary | null>(null);

    useEffect(() => {
        const unlisten = listen<TodoStateEvent>("todo-state", (e) => {
            const event = e.payload;

            switch (event.type) {
                case "todo_state":
                    // Full list state update
                    if (event.summary) {
                        setTodoList(event.summary);
                    }
                    break;

                case "todo_updated":
                    // Single todo updated - recalculate counts
                    if (event.todo) {
                        setTodoList(prev => {
                            if (!prev) return prev;
                            const updatedTodos = prev.todos.map(t =>
                                t.id === event.todo!.id ? event.todo! : t
                            );
                            const completed = updatedTodos.filter(t => t.status === "completed").length;
                            const total = updatedTodos.length;
                            return {
                                ...prev,
                                todos: updatedTodos,
                                completed,
                                total,
                            };
                        });
                    }
                    break;

                case "todo_cleared":
                    // Completed todos cleared - refresh list
                    if (event.list_id && todoList?.list_id === event.list_id) {
                        // The backend will send a full state update after clearing
                        // For now, just mark completed items as removed
                        setTodoList(prev => {
                            if (!prev) return prev;
                            const remainingTodos = prev.todos.filter(t => t.status !== "completed");
                            return {
                                ...prev,
                                todos: remainingTodos,
                                total: remainingTodos.length,
                                completed: 0,
                            };
                        });
                    }
                    break;

                case "todo_list_changed":
                    // New list created - clear current list
                    setTodoList(null);
                    break;
            }
        });

        return () => {
            unlisten.then(fn => fn());
        };
    }, [todoList]);

    if (!todoList || todoList.todos.length === 0) {
        return null;
    }

    const sortedTodos = [...todoList.todos].sort((a, b) => a.position - b.position);

    return (
        <div className="todo-list-container">
            <details className="todo-list-details">
                <summary className="todo-list-summary">
                    <span className="todo-list-icon">📋</span>
                    <span className="todo-list-title">{todoList.title}</span>
                    <span className="todo-list-stats">
                        {todoList.completed}/{todoList.total}
                    </span>
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
