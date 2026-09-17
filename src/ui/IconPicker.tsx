import { useEffect, useMemo, useRef, useState } from "react";
import { GROUPS, loadEmoji, recentIcons, rememberIcon, searchEmoji, suggestIcons, type Emoji } from "./emoji";
import Icon from "./Icon";

interface Props {
  /** What the icon is for, to suggest fitting icons. */
  page: { kind: string; name: string; tags?: string[]; info?: Record<string, string>; notes?: string };
  current: string | null;
  onPick: (icon: string | null) => void;
  onClose: () => void;
  /** Open to the left of its anchor, for anchors near the right edge. */
  align?: "left" | "right";
}

/** Picks an emoji icon: suggestions for the page first, then search, recent and all emoji. */
function IconPicker({ page, current, onPick, onClose, align = "left" }: Props) {
  const [all, setAll] = useState<Emoji[]>([]);
  const [query, setQuery] = useState("");
  const [group, setGroup] = useState(GROUPS[0].id);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    loadEmoji().then(setAll, () => {});
  }, []);

  useEffect(() => {
    const outside = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", outside);
    return () => document.removeEventListener("mousedown", outside);
  }, [onClose]);

  const suggested = useMemo(() => suggestIcons(page, all), [page.name, page.kind, all]);
  const recent = useMemo(recentIcons, []);
  const results = useMemo(() => searchEmoji(all, query).slice(0, 96), [all, query]);
  const shown = query.trim() ? results : all.filter((e) => e.group === group);

  const pick = (icon: string | null) => {
    if (icon) rememberIcon(icon);
    onPick(icon);
    onClose();
  };

  const cell = (emoji: string, label?: string) => (
    <button
      key={emoji}
      type="button"
      className={`emoji-cell${emoji === current ? " current" : ""}`}
      title={label}
      aria-label={label ?? emoji}
      onClick={() => pick(emoji)}
    >
      {emoji}
    </button>
  );

  return (
    <div ref={ref} className={`icon-picker ${align}`} role="dialog" aria-label="Choose an icon" onMouseDown={(e) => e.stopPropagation()}>
      <div className="icon-picker-top">
        <div className="icon-picker-search">
          <Icon name="search" size={14} />
          <input
            autoFocus
            value={query}
            placeholder="Search icons…"
            onChange={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") onClose();
              if (e.key === "Enter" && results[0]) pick(results[0].emoji);
            }}
          />
        </div>
        <button
          type="button"
          className="icon-button"
          title="Surprise me"
          aria-label="Random icon"
          disabled={!all.length}
          onClick={() => {
            const pool = suggested.length > 2 ? suggested : all.map((e) => e.emoji);
            pick(pool[Math.floor(Math.random() * pool.length)]);
          }}
        >
          <Icon name="shuffle" size={15} />
        </button>
        {current && (
          <button type="button" className="button ghost small" onClick={() => pick(null)}>
            Remove
          </button>
        )}
      </div>

      <div className="icon-picker-body">
        {query.trim() ? (
          results.length ? (
            <div className="emoji-grid">{results.map((e) => cell(e.emoji, e.label))}</div>
          ) : (
            <p className="icon-picker-empty">{all.length ? `No icons for "${query}"` : "Loading…"}</p>
          )
        ) : (
          <>
            {suggested.length > 0 && (
              <>
                <div className="icon-picker-label">
                  <Icon name="sparkle" size={11} /> Suggested for {page.name}
                </div>
                <div className="emoji-grid suggested">{suggested.map((e) => cell(e))}</div>
              </>
            )}
            {recent.length > 0 && (
              <>
                <div className="icon-picker-label">Recent</div>
                <div className="emoji-grid">{recent.slice(0, 16).map((e) => cell(e))}</div>
              </>
            )}
            <div className="icon-picker-label">{GROUPS.find((g) => g.id === group)?.label}</div>
            <div className="emoji-grid">{shown.map((e) => cell(e.emoji, e.label))}</div>
          </>
        )}
      </div>

      {!query.trim() && (
        <div className="icon-picker-tabs" role="tablist">
          {GROUPS.map((g) => (
            <button
              key={g.id}
              type="button"
              role="tab"
              aria-selected={g.id === group}
              className={g.id === group ? "on" : ""}
              title={g.label}
              onClick={() => setGroup(g.id)}
            >
              {g.sample}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default IconPicker;
