import { useEffect, useState } from "react";
import { listEntities, openPage, type Entity } from "../api";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";

interface Props {
  version: number;
  onOpen: (id: string) => void;
  onChanged: () => void;
}

const edited = (ms: number) =>
  ms ? new Date(ms).toLocaleDateString(undefined, { day: "numeric", month: "short" }) : "";

/** Free-standing notes, most recently edited first. */
function NotesView({ version, onOpen, onChanged }: Props) {
  const [notes, setNotes] = useState<Entity[] | null>(null);
  const [title, setTitle] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listEntities("Note").then(
      (list) => setNotes(list.sort((a, b) => b.updated_at - a.updated_at)),
      (err) => setError(String(err)),
    );
  }, [version]);

  async function create() {
    const name = title?.trim();
    setTitle(null);
    if (!name) return;
    try {
      const page = await openPage(name);
      onChanged();
      onOpen(page.id);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <h1>Notes</h1>
          <p className="muted">Also in your Obsidian vault.</p>
        </div>
        {title === null && (
          <button className="button primary" onClick={() => setTitle("")}>
            <Icon name="plus" size={14} />
            New note
          </button>
        )}
      </div>
      {title !== null && (
        <input
          className="new-page-input"
          value={title}
          autoFocus
          placeholder="Title, then Enter"
          onChange={(e) => setTitle(e.currentTarget.value)}
          onBlur={create}
          onKeyDown={(e) => {
            if (e.key === "Enter") create();
            if (e.key === "Escape") setTitle(null);
          }}
        />
      )}
      {error && <p className="form-error">{error}</p>}
      {notes && notes.length === 0 && title === null && (
        <p className="empty">No notes yet. Meeting notes, reading lists, ideas: anything goes.</p>
      )}
      {notes && notes.length > 0 && (
        <ul className="list">
          {notes.map((n) => (
            <li key={n.id} className="row clickable" onClick={() => onOpen(n.id)}>
              <Avatar entity={n} size={30} />
              <span className="row-main">
                <span className="row-title">{n.name}</span>
                {n.notes && <span className="row-sub">{n.notes.replace(/\s+/g, " ").slice(0, 90)}</span>}
              </span>
              <span className="row-meta">{edited(n.updated_at)}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export default NotesView;
