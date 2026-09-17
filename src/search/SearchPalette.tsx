import { useEffect, useRef, useState } from "react";
import { openPage, search, type EntityKind, type Hit } from "../api";
import { keyLabel } from "../format";
import type { Route } from "../types";
import { applyAppearance, isDark, loadAppearance } from "../ui/appearance";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";
import { subtitle, typeLabel } from "../ui/entity";

interface Props {
  onOpen: (id: string) => void;
  onSelect: (route: Route) => void;
  onClose: () => void;
  onChanged: () => void;
}

interface Item {
  key: string;
  icon: React.ReactNode;
  title: string;
  kind?: string;
  sub?: string;
  run: () => void | Promise<void>;
}

const GO: { label: string; route: Route; icon: string; words: string }[] = [
  { label: "Today", route: { view: "today" }, icon: "today", words: "today tasks plan" },
  { label: "Chat", route: { view: "chat" }, icon: "chat", words: "chat claude ask" },
  { label: "Updates", route: { view: "updates" }, icon: "bell", words: "updates following papers" },
  { label: "Notes", route: { view: "notes" }, icon: "notes", words: "notes" },
  { label: "Settings", route: { view: "settings" }, icon: "settings", words: "settings preferences appearance theme" },
];

const CREATE: { kind: EntityKind; icon: string }[] = [
  { kind: "Task", icon: "task" },
  { kind: "Project", icon: "project" },
  { kind: "Person", icon: "person" },
  { kind: "Note", icon: "notes" },
];

/** ⌘K: find any page, go anywhere, or create a page from what's typed. */
function SearchPalette({ onOpen, onSelect, onClose, onChanged }: Props) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [selected, setSelected] = useState(0);
  const latest = useRef("");
  const listRef = useRef<HTMLUListElement>(null);

  useEffect(() => {
    latest.current = query;
    if (!query.trim()) {
      setHits([]);
      setSelected(0);
      return;
    }
    const timer = window.setTimeout(() => {
      search(query).then((result) => {
        if (latest.current === query) {
          setHits(result);
          setSelected(0);
        }
      });
    }, 80);
    return () => window.clearTimeout(timer);
  }, [query]);

  const text = query.trim();
  const lower = text.toLowerCase();
  const exact = hits.some((h) => h.entity.name.toLowerCase() === lower);

  const pages: Item[] = hits.map((hit) => ({
    key: hit.entity.id,
    icon: <Avatar entity={hit.entity} size={32} />,
    title: hit.entity.name,
    kind: typeLabel(hit.entity),
    sub: hit.snippet
      ? hit.field === "alias"
        ? `Also called ${hit.snippet}${subtitle(hit.entity) ? ` · ${subtitle(hit.entity)}` : ""}`
        : `${hit.field === "notes" || hit.field === "tag" ? "" : `${keyLabel(hit.field)}: `}${hit.snippet}`
      : [hit.entity.info.full_name !== hit.entity.name && hit.entity.info.full_name, subtitle(hit.entity), hit.entity.info.email]
          .filter(Boolean)
          .join(" · "),
    run: () => onOpen(hit.entity.id),
  }));

  const create: Item[] =
    text && !exact
      ? CREATE.map((c) => ({
          key: `new-${c.kind}`,
          icon: (
            <span className="avatar new">
              <Icon name={c.icon} size={15} />
            </span>
          ),
          title: `New ${c.kind.toLowerCase()} "${text}"`,
          run: async () => {
            const page = await openPage(text, c.kind);
            onChanged();
            onOpen(page.id);
          },
        }))
      : [];

  const dark = isDark();
  const actions: Item[] = [
    ...GO.map((g) => ({
      key: `go-${g.label}`,
      icon: (
        <span className="avatar action">
          <Icon name={g.icon} size={15} />
        </span>
      ),
      title: `Go to ${g.label}`,
      words: g.words,
      run: () => onSelect(g.route),
    })),
    {
      key: "theme",
      icon: (
        <span className="avatar action">
          <Icon name={dark ? "sun" : "moon"} size={15} />
        </span>
      ),
      title: dark ? "Switch to light mode" : "Switch to dark mode",
      words: "theme dark light mode appearance",
      run: () => applyAppearance({ ...loadAppearance(), theme: dark ? "light" : "dark" }),
    },
  ].filter((a) => !text || `${a.title} ${a.words}`.toLowerCase().includes(lower));

  // Pages first, then creating one from the text, then commands.
  const groups: [string, Item[]][] = (
    [
      ["Pages", pages],
      ["Create", create],
      [text ? "Commands" : "Jump to", actions],
    ] as [string, Item[]][]
  ).filter(([, items]) => items.length > 0);
  const items = groups.flatMap(([, g]) => g);

  useEffect(() => {
    listRef.current?.querySelector(".selected")?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  async function choose(index: number) {
    const item = items[index];
    if (!item) return;
    onClose();
    await item.run();
  }

  let index = -1;
  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="palette" role="dialog" aria-label="Search" onMouseDown={(e) => e.stopPropagation()}>
        <div className="palette-input">
          <Icon name="search" size={18} />
          <input
            autoFocus
            value={query}
            placeholder="Search pages, or type a command…"
            onChange={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") onClose();
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setSelected((s) => Math.min(s + 1, items.length - 1));
              }
              if (e.key === "ArrowUp") {
                e.preventDefault();
                setSelected((s) => Math.max(s - 1, 0));
              }
              if (e.key === "Enter" && items.length > 0) {
                e.preventDefault();
                choose(selected);
              }
            }}
          />
          <kbd>esc</kbd>
        </div>
        <ul className="palette-results" ref={listRef}>
          {groups.map(([label, group]) => (
            <li key={label} className="palette-group">
              <div className="palette-group-label">{label}</div>
              <ul>
                {group.map((item) => {
                  index += 1;
                  const i = index;
                  return (
                    <li
                      key={item.key}
                      className={i === selected ? "selected" : ""}
                      onMouseEnter={() => setSelected(i)}
                      onClick={() => choose(i)}
                    >
                      {item.icon}
                      <div className="palette-text">
                        <div className="palette-line">
                          <span className="palette-name">{item.title}</span>
                          {item.kind && <span className="palette-kind">{item.kind}</span>}
                        </div>
                        {item.sub && <div className="palette-snippet">{item.sub}</div>}
                      </div>
                      {i === selected && <kbd className="palette-enter">↵</kbd>}
                    </li>
                  );
                })}
              </ul>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

export default SearchPalette;
