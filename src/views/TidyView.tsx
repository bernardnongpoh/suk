import { useEffect, useState } from "react";
import {
  keepType,
  mergePages,
  notDuplicates,
  removeRelationType,
  renameType,
  tidyItems,
  type DuplicatePair,
  type NewType,
  type TidyItems,
} from "../api";
import Avatar from "../ui/Avatar";

interface Props {
  version: number;
  onOpen: (id: string) => void;
  onChanged: () => void;
}

/**
 * What Suk noticed and wants a decision on: relationships and page types it started using because
 * a message needed them, and pages that look like the same thing twice. Nothing here changes
 * until you choose.
 */
function TidyView({ version, onOpen, onChanged }: Props) {
  const [items, setItems] = useState<TidyItems | null>(null);
  const [renaming, setRenaming] = useState<{ what: "relation" | "kind"; name: string; value: string } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => tidyItems().then(setItems, (err) => setError(String(err)));
  useEffect(() => {
    load();
  }, [version]);

  async function act(key: string, action: () => Promise<unknown>) {
    setBusy(key);
    setError(null);
    try {
      await action();
      await load();
      onChanged();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  const typeRow = (what: "relation" | "kind", type: NewType) => {
    const key = `${what}:${type.name}`;
    const editing = renaming?.what === what && renaming.name === type.name;
    const save = () => {
      const value = renaming?.value.trim();
      setRenaming(null);
      if (value && value !== type.name) act(key, () => renameType(what, type.name, value));
    };
    return (
      <li key={key} className="tidy-item">
        <div className="tidy-main">
          <div className="tidy-title">
            {what === "relation" ? <code>{type.name}</code> : type.name}
            <span className="muted small">
              {type.count} {what === "relation" ? (type.count === 1 ? "relationship" : "relationships") : type.count === 1 ? "page" : "pages"}
            </span>
          </div>
          <div className="muted small">{type.examples.join(" · ")}</div>
          {editing && (
            <input
              className="detail-input"
              autoFocus
              value={renaming.value}
              placeholder={what === "relation" ? "REVIEWS, or an existing one to merge into" : "Grant, or an existing type"}
              onChange={(e) => setRenaming({ ...renaming, value: e.currentTarget.value })}
              onBlur={save}
              onKeyDown={(e) => {
                if (e.key === "Enter") save();
                if (e.key === "Escape") setRenaming(null);
              }}
            />
          )}
        </div>
        <div className="tidy-actions">
          <button className="button small" disabled={busy === key} onClick={() => act(key, () => keepType(what, type.name))}>
            Keep
          </button>
          <button className="button small ghost" disabled={busy === key} onClick={() => setRenaming({ what, name: type.name, value: type.name })}>
            Rename or merge
          </button>
          {what === "relation" && (
            <button className="button small ghost danger-text" disabled={busy === key} onClick={() => act(key, () => removeRelationType(type.name))}>
              Remove
            </button>
          )}
        </div>
      </li>
    );
  };

  const duplicateRow = (pair: DuplicatePair) => {
    const key = `dup:${pair.keep.id}:${pair.remove.id}`;
    return (
      <li key={key} className="tidy-item">
        <div className="tidy-main">
          <div className="tidy-title">
            <button className="task-chip" onClick={() => onOpen(pair.keep.id)}>
              <Avatar entity={pair.keep} size={16} />
              {pair.keep.name}
            </button>
            <span className="muted small">and</span>
            <button className="task-chip" onClick={() => onOpen(pair.remove.id)}>
              <Avatar entity={pair.remove} size={16} />
              {pair.remove.name}
            </button>
          </div>
          <div className="muted small">{pair.reason}</div>
        </div>
        <div className="tidy-actions">
          <button className="button small" disabled={busy === key} onClick={() => act(key, () => mergePages(pair.keep.id, pair.remove.id))}>
            Merge into {pair.keep.name}
          </button>
          <button className="button small ghost" disabled={busy === key} onClick={() => act(key, () => notDuplicates(pair.keep.id, pair.remove.id))}>
            {pair.keep.kind === "Person" ? "Different people" : "Different things"}
          </button>
        </div>
      </li>
    );
  };

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <h1>Tidy up</h1>
          <p className="muted">New ways Suk started organizing things, and pages that may be the same. Nothing changes until you choose.</p>
        </div>
      </div>

      {error && <p className="form-error">{error}</p>}

      {items && items.count === 0 && (
        <div className="empty calm">
          <div className="empty-emoji">✨</div>
          <p>Nothing to tidy. New relationships, page types and possible duplicates will show up here.</p>
        </div>
      )}

      {items && items.relations.length > 0 && (
        <section>
          <h2 className="section-title">
            New relationships <span className="count">{items.relations.length}</span>
          </h2>
          <ul className="tidy-list">{items.relations.map((t) => typeRow("relation", t))}</ul>
        </section>
      )}

      {items && items.kinds.length > 0 && (
        <section>
          <h2 className="section-title">
            New kinds of page <span className="count">{items.kinds.length}</span>
          </h2>
          <ul className="tidy-list">{items.kinds.map((t) => typeRow("kind", t))}</ul>
        </section>
      )}

      {items && items.duplicates.length > 0 && (
        <section>
          <h2 className="section-title">
            Possibly the same <span className="count">{items.duplicates.length}</span>
          </h2>
          <ul className="tidy-list">{items.duplicates.map(duplicateRow)}</ul>
        </section>
      )}
    </div>
  );
}

export default TidyView;
