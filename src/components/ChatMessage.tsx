import type { Message } from "../types";
import "./ChatMessage.css";

interface Props {
  message: Message;
}

export function ChatMessage({ message }: Props) {
  const isUser = message.role === "user";
  return (
    <div className={`chat-message ${isUser ? "user" : "assistant"}`}>
      <div className="message-role">{isUser ? "You" : "Assistant"}</div>
      <div className="message-content">
        {message.content}
        {message.streaming && <span className="cursor">▋</span>}
      </div>
    </div>
  );
}
