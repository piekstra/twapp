interface Props {
  /** The row it confirms for; the confirm row takes the same background. */
  className?: string;
  columns: number;
  /** Names what stops, as in "Stop {name}". */
  name: string;
  explain: string;
  onCancel: () => void;
  onStop: () => void;
}

/**
 * Asks before a stop in its own row under the one it stops, so opening it
 * does not resize the table's columns.
 */
export default function StopConfirmRow({ className, columns, name, explain, onCancel, onStop }: Props) {
  return (
    <tr className={`resources-confirm-row${className ? ` ${className}` : ""}`}>
      <td colSpan={columns}>
        <div className="resources-confirm" role="group" aria-label={`Stop ${name}?`}>
          <span className="board-muted">{explain}</span>
          <button className="button ghost small" data-focus="cancel" onClick={onCancel} aria-label={`Cancel stopping ${name}`}>
            Cancel
          </button>
          <button className="button ghost small danger" onClick={onStop} aria-label={`Stop ${name}`}>
            Stop
          </button>
        </div>
      </td>
    </tr>
  );
}
