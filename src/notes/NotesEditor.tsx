import { useEffect, useRef, useState } from "react";
import { saveNotes, search, type Entity, type Hit } from "../api";
import Markdown from "./Markdown";

interface Props {
  page: Entity;
  onLink: (name: string) => void;
  onSaved: (page: Entity) => void;
}

/** The [[link text being typed just before the caret, if any. */
function openLink(text: string, caret: number) {
  const before = text.slice(0, caret);
  const start = before.lastIndexOf("[[");
  if (start < 0 || before.slice(start).includes("]]") || before.slice(start).includes("\n")) return null;
  return { start: start + 2, query: before.slice(start + 2) };
}

/**
 * A page's notes, shown as Markdown. Clicking them edits the text; it saves as you type and when
 * you leave. Typing [[ suggests pages to link.
 */
function NotesEditor({ page, onLink, onSaved }: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(page.notes);
  const [status, setStatus] = useState<"saved" | "saving" | "error" | null>(null);
  const [suggestions, setSuggestions] = useState<Hit[]>([]);
  const [linkAt, setLinkAt] = useState<{ start: number; query: string } | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const saved = useRef(page.notes);
  const timer = useRef<number | undefined>(undefined);

  // Changes from Obsidian or Claude show up unless the notes are being edited here.
  useEffect(() => {
    if (!editing) {
      setDraft(page.notes);
      saved.current = page.notes;
    }
  }, [page.notes, editing]);

  useEffect(() => {
    if (editing && area.current) {
      area.current.style.height = "auto";
      area.current.style.height = `${area.current.scrollHeight + 2}px`;
    }
  }, [draft, editing]);

  async function save(text: string) {
    window.clearTimeout(timer.current);
    if (text === saved.current) return;
    setStatus("saving");
    try {
      const updated = await saveNotes(page.id, text);
      saved.current = text;
      setStatus("saved");
      onSaved(updated);
    } catch {
      setStatus("error");
    }
  }

  function change(text: string, caret: number) {
    setDraft(text);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => save(text), 800);
    const link = openLink(text, caret);
    setLinkAt(link);
    if (link && link.query.trim()) {
      search(link.query).then((hits) => setSuggestions(hits.slice(0, 6)), () => setSuggestions([]));
    } else {
      setSuggestions([]);
    }
  }

  function insertLink(name: string) {
    if (!linkAt || !area.current) return;
    const caret = area.current.selectionStart;
    const after = draft.slice(caret).startsWith("]]") ? draft.slice(caret + 2) : draft.slice(caret);
    const text = `${draft.slice(0, linkAt.start)}${name}]]${after}`;
    const position = linkAt.start + name.length + 2;
    change(text, position);
    setLinkAt(null);
    setSuggestions([]);
    requestAnimationFrame(() => area.current?.setSelectionRange(position, position));
    area.current.focus();
  }

  if (!editing) {
    return (
      <div className="notes" onClick={() => setEditing(true)} title="Click to edit">
        {draft.trim() ? (
          <Markdown text={draft} onLink={onLink} />
        ) : (
          <p className="notes-placeholder">
            Write anything about {page.name}. Use [[ to link pages and # for tags.
          </p>
        )}
      </div>
    );
  }

  return (
    <div className="notes editing">
      <textarea
        ref={area}
        value={draft}
        autoFocus
        spellCheck
        placeholder={`Write anything about ${page.name}…`}
        onChange={(e) => change(e.currentTarget.value, e.currentTarget.selectionStart)}
        onBlur={() => {
          // Let a click on a suggestion land first.
          window.setTimeout(() => {
            if (document.activeElement !== area.current) {
              save(area.current?.value ?? draft);
              setEditing(false);
              setSuggestions([]);
            }
          }, 150);
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") area.current?.blur();
          if (e.key === "Enter" && suggestions.length > 0) {
            e.preventDefault();
            insertLink(suggestions[0].entity.name);
          }
        }}
      />
      {linkAt && linkAt.query.trim() && (
        <div className="link-suggestions">
          {suggestions.map((hit) => (
            <button key={hit.entity.id} onMouseDown={(e) => e.preventDefault()} onClick={() => insertLink(hit.entity.name)}>
              {hit.entity.name} <span className="muted">{hit.entity.kind}</span>
            </button>
          ))}
          {!suggestions.some((h) => h.entity.name.toLowerCase() === linkAt.query.trim().toLowerCase()) && (
            <button onMouseDown={(e) => e.preventDefault()} onClick={() => insertLink(linkAt.query.trim())}>
              New page "{linkAt.query.trim()}"
            </button>
          )}
        </div>
      )}
      <div className="notes-status">
        {status === "saving" ? "Saving…" : status === "error" ? "Couldn't save" : status === "saved" ? "Saved" : "Markdown · [[ links pages · Esc to finish"}
      </div>
    </div>
  );
}

export default NotesEditor;
