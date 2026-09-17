import { useEffect, useRef, useState } from "react";
import { fillProfile, openUrl, updateDetails, type DetailField, type Entity } from "../api";
import { dueLabel, keyLabel } from "../format";
import Icon from "../ui/Icon";

interface Props {
  entity: Entity;
  /** The details this kind of page usually has, in order. */
  fields: DetailField[];
  /** Details shown or edited elsewhere on the page. */
  hidden: (key: string) => boolean;
  /** Rows that aren't plain details, such as a person's organizations. */
  before?: React.ReactNode;
  /** People: offer filling details in from a profile link. */
  profileLink?: boolean;
  onSaved: () => void;
}

/** Icons for well-known details. */
const ICONS: Record<string, string> = {
  email: "mail",
  phone: "chat",
  position: "briefcase",
  affiliation: "building",
  department: "building",
  homepage: "globe",
  url: "link",
  program: "student",
  start: "calendar",
  end: "calendar",
  thesis: "notes",
  due: "calendar",
  priority: "flag",
  status: "check",
  type: "building",
  city: "globe",
  country: "globe",
  location: "globe",
  full_name: "person",
  funding: "hash",
  code: "hash",
  semester: "calendar",
  schedule: "calendar",
  room: "building",
};

const TEXT_FIELD: Omit<DetailField, "key" | "label"> = { input: "text", options: [] };

/** "Grant number" -> "grant_number", the form details are stored in. */
const toKey = (name: string) =>
  name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .replace(/^(\d)/, "_$1");

const isUrl = (value: string) => /^https?:\/\/\S+$/.test(value);

/**
 * A page's details, each editable in place: click a value to change it, clear it to remove it, or
 * add one from the details this kind of page usually has. Saved at once to the page, its
 * Markdown file, and what the assistant knows.
 */
function Details({ entity, fields, hidden, before, profileLink, onSaved }: Props) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  // A detail being added that has no value yet.
  const [adding, setAdding] = useState<string | null>(null);
  const [menu, setMenu] = useState(false);
  const [customName, setCustomName] = useState<string | null>(null);
  const [link, setLink] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<{ key: string; message: string } | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!saved) return;
    const timer = setTimeout(() => setSaved(null), 1600);
    return () => clearTimeout(timer);
  }, [saved]);

  useEffect(() => {
    if (!menu) return;
    const close = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenu(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [menu]);

  const fieldFor = (key: string): DetailField => fields.find((f) => f.key === key) ?? { key, label: keyLabel(key), ...TEXT_FIELD };
  const order = (key: string) => {
    const i = fields.findIndex((f) => f.key === key);
    return i === -1 ? fields.length : i;
  };
  const keys = Object.keys(entity.info)
    .filter((key) => !hidden(key))
    .sort((a, b) => order(a) - order(b) || a.localeCompare(b));
  if (adding && !keys.includes(adding)) keys.push(adding);
  const suggestions = fields.filter((f) => !hidden(f.key) && !(f.key in entity.info) && f.key !== adding);

  function start(key: string) {
    setEditing(key);
    setDraft(entity.info[key] ?? "");
    setError(null);
  }

  function cancel() {
    setEditing(null);
    setAdding(null);
    setError(null);
  }

  async function save(key: string, value: string | null) {
    const current = entity.info[key] ?? null;
    const next = value?.trim() ? value.trim() : null;
    if (next === current) {
      cancel();
      return;
    }
    setBusy(key);
    try {
      await updateDetails(entity.id, { [key]: next });
      setEditing(null);
      setAdding(null);
      setError(null);
      setSaved(key);
      onSaved();
    } catch (err) {
      setError({ key, message: String(err).replace(/^Error: /, "") });
    } finally {
      setBusy(null);
    }
  }

  function add(key: string) {
    setMenu(false);
    setAdding(key);
    start(key);
  }

  async function fill() {
    const url = link?.trim();
    if (!url) return;
    setBusy("link");
    setError(null);
    try {
      await fillProfile(entity.id, url, () => {});
      setLink(null);
      setSaved("link");
      onSaved();
    } catch (err) {
      setError({ key: "link", message: String(err) });
    } finally {
      setBusy(null);
    }
  }

  const editor = (field: DetailField) => {
    const common = {
      autoFocus: true,
      disabled: busy === field.key,
      "aria-label": field.label,
      onKeyDown: (e: React.KeyboardEvent) => {
        if (e.key === "Escape") cancel();
        if (e.key === "Enter") {
          e.preventDefault();
          save(field.key, draft);
        }
      },
    };
    switch (field.input) {
      case "choice":
        return (
          <select
            className="detail-input"
            {...common}
            value={draft}
            onChange={(e) => save(field.key, e.currentTarget.value || null)}
            onBlur={() => editing === field.key && cancel()}
          >
            <option value="">—</option>
            {field.options.map((o) => (
              <option key={o} value={o}>
                {keyLabel(o)}
              </option>
            ))}
          </select>
        );
      case "due": {
        const [date, time] = draft.split("T");
        const set = (d: string, t: string | undefined) => setDraft(d ? (t ? `${d}T${t}` : d) : "");
        return (
          <span
            className="detail-due"
            onBlur={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget as Node)) save(field.key, draft);
            }}
          >
            <input className="detail-input" type="date" {...common} value={date ?? ""} onChange={(e) => set(e.currentTarget.value, time)} />
            <input
              className="detail-input time"
              type="time"
              {...common}
              autoFocus={false}
              aria-label={`${field.label} time`}
              value={time ?? ""}
              onChange={(e) => set(date ?? "", e.currentTarget.value)}
            />
          </span>
        );
      }
      case "datetime":
        return (
          <input
            className="detail-input"
            type="datetime-local"
            {...common}
            value={draft}
            onChange={(e) => setDraft(e.currentTarget.value)}
            onBlur={() => save(field.key, draft)}
          />
        );
      default:
        return (
          <>
            <input
              className="detail-input"
              type={field.input === "email" ? "email" : field.input === "url" ? "url" : "text"}
              {...common}
              value={draft}
              placeholder={field.options.slice(0, 3).join(", ") || field.label}
              list={field.options.length ? `detail-${field.key}` : undefined}
              onChange={(e) => setDraft(e.currentTarget.value)}
              onBlur={() => save(field.key, draft)}
            />
            {field.options.length > 0 && (
              <datalist id={`detail-${field.key}`}>
                {field.options.map((o) => (
                  <option key={o} value={o} />
                ))}
              </datalist>
            )}
          </>
        );
    }
  };

  const shown = (field: DetailField, value: string) => {
    if (field.key === "due") return `${dueLabel(value)} · ${value.replace("T", " ")}`;
    if (field.input === "choice") return keyLabel(value);
    if (isUrl(value)) return value.replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, "");
    return value;
  };

  return (
    <div className="details">
      {(keys.length > 0 || before || customName !== null) && (
      <dl className="props">
        {before}
        {keys.map((key) => {
          const field = fieldFor(key);
          const value = entity.info[key] ?? "";
          return (
            <div key={key} className={`prop${editing === key ? " editing" : ""}`}>
              <dt>
                <Icon name={ICONS[key] ?? "hash"} size={14} />
                {field.label}
              </dt>
              <dd>
                {editing === key ? (
                  editor(field)
                ) : (
                  <span className="detail-value">
                    <button className="detail-edit" title={`Edit ${field.label.toLowerCase()}`} onClick={() => start(key)}>
                      {shown(field, value)}
                    </button>
                    {isUrl(value) && (
                      <button className="icon-button tiny" aria-label={`Open ${field.label.toLowerCase()}`} onClick={() => openUrl(value)}>
                        <Icon name="external" size={12} />
                      </button>
                    )}
                    <button className="icon-button tiny remove" aria-label={`Remove ${field.label.toLowerCase()}`} onClick={() => save(key, null)}>
                      <Icon name="close" size={12} />
                    </button>
                    {saved === key && (
                      <span className="detail-saved">
                        <Icon name="check" size={12} /> Saved
                      </span>
                    )}
                  </span>
                )}
                {error?.key === key && <div className="form-error small">{error.message}</div>}
              </dd>
            </div>
          );
        })}
        {customName !== null && (
          <div className="prop editing">
            <dt>
              <Icon name="hash" size={14} />
              New detail
            </dt>
            <dd>
              <input
                className="detail-input"
                autoFocus
                value={customName}
                placeholder="Name, e.g. Grant number"
                onChange={(e) => setCustomName(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setCustomName(null);
                  if (e.key === "Enter" && toKey(customName)) {
                    add(toKey(customName));
                    setCustomName(null);
                  }
                }}
                onBlur={() => {
                  if (toKey(customName)) add(toKey(customName));
                  setCustomName(null);
                }}
              />
            </dd>
          </div>
        )}
      </dl>
      )}

      <div className="details-actions" ref={menuRef}>
        <span className="menu-anchor">
          <button className="text-button" onClick={() => setMenu((m) => !m)}>
            <Icon name="plus" size={13} /> Add detail
          </button>
          {menu && (
            <div className="menu" role="menu">
              {suggestions.map((f) => (
                <button key={f.key} role="menuitem" onClick={() => add(f.key)}>
                  <Icon name={ICONS[f.key] ?? "hash"} size={14} /> {f.label}
                </button>
              ))}
              <button role="menuitem" onClick={() => { setMenu(false); setCustomName(""); }}>
                <Icon name="pencil" size={14} /> Other…
              </button>
            </div>
          )}
        </span>
        {profileLink &&
          (link === null ? (
            <button className="text-button" onClick={() => setLink("")}>
              <Icon name="sparkle" size={13} /> Fill in from a profile link
            </button>
          ) : (
            <span className="details-link">
              <input
                className="detail-input"
                autoFocus
                type="url"
                value={link}
                disabled={busy === "link"}
                placeholder="University page, personal site…"
                onChange={(e) => setLink(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") fill();
                  if (e.key === "Escape") setLink(null);
                }}
              />
              {busy === "link" ? (
                <span className="busy">
                  <span className="spinner" /> Reading…
                </span>
              ) : (
                <button className="button small" disabled={!link.trim()} onClick={fill}>
                  Fill in
                </button>
              )}
            </span>
          ))}
        {saved === "link" && (
          <span className="detail-saved">
            <Icon name="check" size={12} /> Filled in
          </span>
        )}
      </div>
      {error?.key === "link" && <p className="form-error small">{error.message}</p>}
    </div>
  );
}

export default Details;
