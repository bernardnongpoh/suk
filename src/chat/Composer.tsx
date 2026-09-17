import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { search, type Entity } from "../api";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";
import { subtitle, typeLabel } from "../ui/entity";

interface Props {
  placeholder: string;
  disabled: boolean;
  autoFocus?: boolean;
  /** Text to put in the box, e.g. from a suggestion; replaces what's there. */
  preset?: { text: string } | null;
  onSend: (text: string, mentions: Entity[]) => void;
}

interface Suggestion {
  /** Where the replaced text starts; it ends at the caret. */
  start: number;
  query: string;
  hits: Entity[];
  /** Where the word it was found for starts. */
  wordStart: number;
}

// Lowercase words too common to look up as names.
const COMMON = new Set(
  "with will have that this from what when they them their there about work working would could should today tomorrow meeting also just need needs plan some your into over after before paper review draft make made more most much very were been being here".split(" "),
);

/** The name being typed before the caret: up to three words, e.g. "Kavya R". */
function nameAt(text: string, caret: number) {
  const before = text.slice(0, caret);
  const match = /(?:^|[\s(,;:"'])((?:[\p{L}][\p{L}\p{N}.'-]*\s){0,2}@?[\p{L}][\p{L}\p{N}.'-]*)$/u.exec(before);
  if (!match) return null;
  const phrase = match[1];
  const words = phrase.split(/\s+/);
  const last = words[words.length - 1].replace(/^@/, "");
  const explicit = words[words.length - 1].startsWith("@");
  const capitalized = /^\p{Lu}/u.test(last);
  if (!explicit && !(capitalized && last.length >= 2) && !(last.length >= 4 && !COMMON.has(last.toLowerCase()))) {
    return null;
  }
  return { phrase, words, last, start: caret - phrase.length };
}

/** Whether a page's name or full name has a word starting with `word`. */
const matchesWord = (e: Entity, word: string) =>
  [e.name, e.info.full_name ?? "", ...(e.aliases ?? [])].some((n) =>
    n.toLowerCase().split(/[\s,]+/).some((w) => w.startsWith(word.toLowerCase())),
  );

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/**
 * The chat box. As a name is typed, matching pages are shown with a few details; Tab, Enter or a
 * click picks one, which highlights it and tells Claude exactly who or what is meant. Esc closes
 * the suggestions, and Enter then sends.
 */
function Composer({ placeholder, disabled, autoFocus, preset, onSend }: Props) {
  const [text, setText] = useState("");
  const [mentions, setMentions] = useState<Entity[]>([]);
  const [suggestion, setSuggestion] = useState<Suggestion | null>(null);
  const [selected, setSelected] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const backdrop = useRef<HTMLDivElement>(null);
  const request = useRef(0);

  useEffect(() => {
    if (preset) {
      setText(preset.text);
      requestAnimationFrame(() => {
        area.current?.focus();
        area.current?.setSelectionRange(preset.text.length, preset.text.length);
      });
    }
  }, [preset]);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
  }, [text]);

  function lookUp(value: string, caret: number) {
    const name = nameAt(value, caret);
    const wordStart = name ? caret - name.last.length : -1;
    // Narrow or drop the shown suggestions right away, before the new search returns, so Enter
    // can't pick one that no longer fits what was typed ("Sat" -> "Saturday" drops Satya).
    setSuggestion((s) => {
      if (!s || !name || s.wordStart !== wordStart) return null;
      const hits = s.hits.filter((e) => matchesWord(e, name.last));
      return hits.length ? { ...s, hits, query: name.phrase } : null;
    });
    setSelected(0);
    if (!name) {
      request.current++;
      return;
    }
    const id = ++request.current;
    search(name.last).then((hits) => {
      if (id !== request.current) return;
      const people = hits
        .map((h) => h.entity)
        .filter((e) => matchesWord(e, name.last))
        .filter((e) => !mentions.some((m) => m.id === e.id && name.phrase.toLowerCase().endsWith(e.name.toLowerCase())))
        .slice(0, 5);
      if (!people.length || `${name.start}:${name.phrase}` === dismissed) {
        setSuggestion(null);
        return;
      }
      // Replace as many of the typed words as the best match starts with ("Kavya R" -> "Kavya Rao").
      let start = caret - name.last.length - (name.phrase.endsWith(`@${name.last}`) ? 1 : 0);
      for (let n = name.words.length; n > 1; n--) {
        const tail = name.words.slice(-n).join(" ").replace(/@/, "").toLowerCase();
        if (people.some((p) => p.name.toLowerCase().startsWith(tail))) {
          start = caret - name.words.slice(-n).join(" ").length;
          break;
        }
      }
      setSuggestion({ start, query: name.phrase, hits: people, wordStart });
      setSelected(0);
    });
  }

  function change(value: string, caret: number) {
    setText(value);
    // Forget picks whose names were deleted.
    setMentions((m) => m.filter((e) => value.toLowerCase().includes(e.name.toLowerCase())));
    lookUp(value, caret);
  }

  function pick(entity: Entity) {
    const el = area.current;
    if (!el || !suggestion) return;
    const caret = el.selectionStart;
    const value = `${text.slice(0, suggestion.start)}${entity.name} ${text.slice(caret)}`;
    const position = suggestion.start + entity.name.length + 1;
    setText(value);
    setMentions((m) => (m.some((e) => e.id === entity.id) ? m : [...m, entity]));
    setSuggestion(null);
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(position, position);
    });
  }

  function send() {
    const value = text.trim();
    if (!value || disabled) return;
    onSend(value, mentions.filter((m) => value.toLowerCase().includes(m.name.toLowerCase())));
    setText("");
    setMentions([]);
    setSuggestion(null);
  }

  // The highlighted copy of the text behind the transparent textarea.
  let highlighted = escapeHtml(text);
  for (const m of [...mentions].sort((a, b) => b.name.length - a.name.length)) {
    const pattern = new RegExp(escapeHtml(m.name).replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
    highlighted = highlighted.replace(pattern, (s) => `<mark>${s}</mark>`);
  }

  return (
    <div className="composer">
      {suggestion && (
        <div className="mention-popover" role="listbox" aria-label="Pages matching what you typed">
          {suggestion.hits.map((entity, i) => (
            <button
              key={entity.id}
              role="option"
              aria-selected={i === selected}
              className={`mention-option${i === selected ? " selected" : ""}`}
              onMouseDown={(e) => e.preventDefault()}
              onMouseEnter={() => setSelected(i)}
              onClick={() => pick(entity)}
            >
              <Avatar entity={entity} size={30} />
              <span className="mention-main">
                <span className="mention-name">
                  {entity.name}
                  {entity.info.full_name && entity.info.full_name !== entity.name && (
                    <span className="mention-full"> {entity.info.full_name}</span>
                  )}
                </span>
                <span className="mention-sub">
                  {[typeLabel(entity), subtitle(entity), entity.info.email].filter(Boolean).join(" · ")}
                </span>
              </span>
              {i === selected && (
                <span className="mention-keys">
                  <kbd>Tab</kbd>
                  <kbd>↵</kbd>
                </span>
              )}
            </button>
          ))}
        </div>
      )}
      <div className="composer-box">
        <div className="composer-field">
          <div ref={backdrop} className="composer-backdrop" aria-hidden="true" dangerouslySetInnerHTML={{ __html: `${highlighted}\n` }} />
          <textarea
            ref={area}
            value={text}
            rows={1}
            placeholder={placeholder}
            autoFocus={autoFocus}
            spellCheck
            onChange={(e) => change(e.currentTarget.value, e.currentTarget.selectionStart)}
            onScroll={(e) => {
              if (backdrop.current) backdrop.current.scrollTop = e.currentTarget.scrollTop;
            }}
            onClick={(e) => lookUp(text, e.currentTarget.selectionStart)}
            onBlur={() => setSuggestion(null)}
            onKeyDown={(e) => {
              if (suggestion) {
                // While names are suggested, Enter picks one like Tab does; the next Enter sends.
                if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing)) {
                  e.preventDefault();
                  pick(suggestion.hits[selected]);
                  return;
                }
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  const n = suggestion.hits.length;
                  setSelected((s) => (s + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  setDismissed(`${suggestion.start}:${suggestion.query}`);
                  setSuggestion(null);
                  return;
                }
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
          />
        </div>
        <button className="send-button" onClick={send} disabled={!text.trim() || disabled} aria-label="Send">
          <Icon name="send" size={16} />
        </button>
      </div>
      {mentions.length > 0 && (
        <div className="composer-mentions">
          {mentions.map((m) => (
            <span key={m.id} className="mention-chip" title={subtitle(m)}>
              <Avatar entity={m} size={16} />
              {m.name}
              <span className="muted">{typeLabel(m)}</span>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

export default Composer;
