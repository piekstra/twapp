import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type MessagePriority = "routine" | "urgent" | "blocker";

export type SendArgs = {
  to: string;
  priority: MessagePriority;
  subject?: string;
  thread?: string;
  cc?: string;
  body: string;
};

type MessageComposerProps = {
  open: boolean;
  onClose: () => void;
  onSent?: (id: string) => void;
  initialTo?: string;
  initialThread?: string;
  initialReplyToSubject?: string;
};

export function validateArgs(args: SendArgs): string | null {
  if (!args.to.trim()) return "Recipient is required (handle or \"all\")";
  if (!args.body.trim()) return "Message body cannot be empty";
  return null;
}

export default function MessageComposer({
  open,
  onClose,
  onSent,
  initialTo = "",
  initialThread = "",
  initialReplyToSubject = "",
}: MessageComposerProps) {
  const [to, setTo] = useState(initialTo);
  const [priority, setPriority] = useState<MessagePriority>("routine");
  const [subject, setSubject] = useState(initialReplyToSubject);
  const [thread, setThread] = useState(initialThread);
  const [cc, setCc] = useState("");
  const [body, setBody] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setTo(initialTo);
    setSubject(initialReplyToSubject);
    setThread(initialThread);
    setPriority("routine");
    setCc("");
    setBody("");
    setError(null);
    setSending(false);
    setTimeout(() => toRef.current?.focus(), 0);
  }, [open, initialTo, initialReplyToSubject, initialThread]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const submit = async () => {
    const args: SendArgs = {
      to: to.trim(),
      priority,
      subject: subject.trim() || undefined,
      thread: thread.trim() || undefined,
      cc: cc.trim() || undefined,
      body,
    };
    const validationError = validateArgs(args);
    if (validationError) {
      setError(validationError);
      return;
    }
    setSending(true);
    setError(null);
    try {
      const id = await invoke<string>("send_message", { args });
      onSent?.(id);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
    }
  };

  const onBodyKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      if (!sending) submit();
    }
  };

  return (
    <div className="composer-overlay" onClick={onClose}>
      <div className="composer-panel" onClick={(e) => e.stopPropagation()}>
        <div className="composer-header">
          <span>Send Message</span>
          <button className="composer-close" onClick={onClose} aria-label="Close">
            x
          </button>
        </div>
        <div className="composer-body">
          <label className="composer-label">
            To
            <input
              ref={toRef}
              type="text"
              className="composer-input"
              placeholder='handle, or "all" for broadcast'
              value={to}
              onChange={(e) => setTo(e.target.value)}
            />
          </label>
          <label className="composer-label">
            Priority
            <select
              className="composer-input"
              value={priority}
              onChange={(e) => setPriority(e.target.value as MessagePriority)}
            >
              <option value="routine">routine</option>
              <option value="urgent">urgent</option>
              <option value="blocker">blocker</option>
            </select>
          </label>
          <label className="composer-label">
            Subject
            <input
              type="text"
              className="composer-input"
              placeholder="optional — one line"
              value={subject}
              onChange={(e) => setSubject(e.target.value)}
            />
          </label>
          <label className="composer-label">
            Thread
            <input
              type="text"
              className="composer-input composer-input-disabled"
              placeholder="prefilled when replying to an inbox message"
              value={thread}
              onChange={(e) => setThread(e.target.value)}
              disabled
              title="Reply-to threading lands in a follow-up PR"
            />
          </label>
          <label className="composer-label">
            cc
            <input
              type="text"
              className="composer-input"
              placeholder="comma-separated handles — optional"
              value={cc}
              onChange={(e) => setCc(e.target.value)}
            />
          </label>
          <label className="composer-label">
            Body
            <textarea
              className="composer-textarea"
              placeholder="Markdown body. Cmd/Ctrl+Enter to send."
              value={body}
              onChange={(e) => setBody(e.target.value)}
              onKeyDown={onBodyKeyDown}
              rows={10}
            />
          </label>
          {error && <div className="composer-error">{error}</div>}
          <div className="composer-actions">
            <button className="composer-cancel" onClick={onClose} disabled={sending}>
              Cancel
            </button>
            <button className="composer-submit" onClick={submit} disabled={sending}>
              {sending ? "Sending..." : "Send"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
