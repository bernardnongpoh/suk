import { useState } from "react";
import { dismissSection, openPage, pinSection, setSectionIcon, type EntityKind, type Sidebar as SidebarData } from "../api";
import type { Route } from "../types";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";
import IconPicker from "../ui/IconPicker";
import { iconForTag } from "../ui/entity";

interface Props {
  route: Route;
  data: SidebarData | null;
  /** Relevant updates from followed people not yet seen. */
  unread: number;
  /** Things Suk organized in a new way and wants a decision on. */
  toTidy: number;
  onSelect: (route: Route) => void;
  onSearch: () => void;
  onOpen: (id: string) => void;
  onChanged: () => void;
}

/** Kinds of page the New menu creates, with their placeholder names. */
const NEW_KINDS: { kind: EntityKind; label: string; icon: string; placeholder: string }[] = [
  { kind: "Task", label: "Task", icon: "task", placeholder: "What needs doing?" },
  { kind: "Project", label: "Project", icon: "project", placeholder: "Project name" },
  { kind: "Person", label: "Person", icon: "person", placeholder: "Their name" },
  { kind: "Course", label: "Course", icon: "course", placeholder: "Course name" },
  { kind: "Idea", label: "Idea", icon: "idea", placeholder: "The idea, in a few words" },
  { kind: "Note", label: "Note", icon: "notes", placeholder: "Note title" },
];

const isMac = navigator.platform.toLowerCase().includes("mac");

/**
 * Today, Chat, Updates and Notes are always there. Everything else is sections the user added,
 * usually from a suggestion after mentioning something new; tags in use without a section are
 * offered at the bottom.
 */
function Sidebar({ route, data, unread, toTidy, onSelect, onSearch, onOpen, onChanged }: Props) {
  const [menu, setMenu] = useState<string | null>(null);
  // The section whose icon is being chosen.
  const [iconFor, setIconFor] = useState<string | null>(null);
  // Creating a page from the New menu: which kind, and its name so far.
  const [newPage, setNewPage] = useState<{ kind: (typeof NEW_KINDS)[number]; name: string } | null>(null);
  const [renaming, setRenaming] = useState<{ tag: string; title: string } | null>(null);
  const [creating, setCreating] = useState<string | null>(null);
  const [showSuggested, setShowSuggested] = useState(false);

  const active = (r: Route) =>
    r.view === route.view && (r.view !== "section" || (route.view === "section" && route.tag === r.tag));

  const item = (r: Route, label: string, icon: string, extra?: React.ReactNode, emoji?: string) => (
    <button key={label} className={`nav-item${active(r) ? " active" : ""}`} onClick={() => onSelect(r)}>
      {emoji ? <span className="nav-emoji">{emoji}</span> : <Icon name={icon} size={16} />}
      <span className="nav-label">{label}</span>
      {extra}
    </button>
  );

  async function saveRename() {
    if (renaming?.title.trim()) await pinSection(renaming.tag, renaming.title.trim());
    setRenaming(null);
    onChanged();
  }

  async function create() {
    const title = creating?.trim();
    setCreating(null);
    if (!title) return;
    await pinSection(title, title);
    onChanged();
  }

  async function createPage() {
    const draft = newPage;
    setNewPage(null);
    const name = draft?.name.trim();
    if (!draft || !name) return;
    const page = await openPage(name, draft.kind.kind);
    onChanged();
    onOpen(page.id);
  }

  const suggested = data?.suggested ?? [];
  const favorites = data?.favorites ?? [];

  return (
    <nav className="sidebar">
      <div className="brand">
        <span className="brand-mark">
          <Icon name="sparkle" size={14} />
        </span>
        <span className="brand-name">Suk</span>
        <span className="menu-anchor">
          <button className="icon-button new-button" aria-label="New page" title="New page" onClick={() => setMenu(menu === "new" ? null : "new")}>
            <Icon name="plus" size={16} />
          </button>
          {menu === "new" && (
            <div className="menu right" role="menu" onMouseLeave={() => setMenu(null)}>
              {NEW_KINDS.map((k) => (
                <button key={k.kind} role="menuitem" onClick={() => { setMenu(null); setNewPage({ kind: k, name: "" }); }}>
                  <Icon name={k.icon} size={14} /> {k.label}
                </button>
              ))}
            </div>
          )}
        </span>
      </div>
      {newPage && (
        <div className="nav-new">
          <Icon name={newPage.kind.icon} size={15} />
          <input
            autoFocus
            value={newPage.name}
            placeholder={newPage.kind.placeholder}
            aria-label={`New ${newPage.kind.label.toLowerCase()}`}
            onChange={(e) => setNewPage({ ...newPage, name: e.currentTarget.value })}
            onBlur={() => (newPage.name.trim() ? createPage() : setNewPage(null))}
            onKeyDown={(e) => {
              if (e.key === "Enter") createPage();
              if (e.key === "Escape") setNewPage(null);
            }}
          />
        </div>
      )}
      <button className="search-button" onClick={onSearch}>
        <Icon name="search" size={15} />
        <span>Search</span>
        <kbd>{isMac ? "⌘K" : "Ctrl K"}</kbd>
      </button>

      <div className="nav-group">
        {item({ view: "today" }, "Today", "today")}
        {item({ view: "chat" }, "Chat", "chat")}
        {item({ view: "calendar" }, "Calendar", "calendar")}
        {item({ view: "updates" }, "Updates", "bell", unread > 0 && <span className="nav-badge" aria-label={`${unread} new`}>{unread}</span>)}
        {item({ view: "notes" }, "Notes", "notes")}
      </div>

      {favorites.length > 0 && (
        <div className="nav-group">
          <div className="nav-heading">Favorites</div>
          {favorites.map((page) => (
            <button
              key={page.id}
              className={`nav-item${route.view === "page" && route.id === page.id ? " active" : ""}`}
              onClick={() => onOpen(page.id)}
            >
              <Avatar entity={page} size={18} />
              <span className="nav-label">{page.name}</span>
            </button>
          ))}
        </div>
      )}

      <div className="nav-group nav-sections">
        {(data?.sections.length ?? 0) > 0 && <div className="nav-heading">Sections</div>}
        {data?.sections.map((s) =>
          renaming?.tag === s.tag ? (
            <input
              key={s.tag}
              className="nav-input"
              value={renaming.title}
              autoFocus
              onChange={(e) => setRenaming({ tag: s.tag, title: e.currentTarget.value })}
              onBlur={saveRename}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveRename();
                if (e.key === "Escape") setRenaming(null);
              }}
            />
          ) : (
            <div key={s.tag} className="nav-row" onMouseLeave={() => menu === s.tag && setMenu(null)}>
              {item({ view: "section", tag: s.tag, title: s.title }, s.title, iconForTag(s.tag), <span className="nav-count">{s.count}</span>, s.icon)}
              <button className="nav-more" aria-label={`${s.title} options`} onClick={() => setMenu(menu === s.tag ? null : s.tag)}>
                <Icon name="more" size={14} />
              </button>
              {iconFor === s.tag && (
                <IconPicker
                  page={{ kind: "Section", name: s.title, tags: [s.tag] }}
                  current={s.icon || null}
                  onPick={(icon) => setSectionIcon(s.tag, icon).then(onChanged)}
                  onClose={() => setIconFor(null)}
                />
              )}
              {menu === s.tag && (
                <div className="menu nav-menu">
                  <button onClick={() => { setMenu(null); setRenaming({ tag: s.tag, title: s.title }); }}>
                    <Icon name="pencil" size={14} /> Rename
                  </button>
                  <button onClick={() => { setMenu(null); setIconFor(s.tag); }}>
                    <Icon name="smile" size={14} /> {s.icon ? "Change icon" : "Choose icon"}
                  </button>
                  <button
                    onClick={async () => {
                      setMenu(null);
                      await dismissSection(s.tag);
                      if (route.view === "section" && route.tag === s.tag) onSelect({ view: "today" });
                      onChanged();
                    }}
                  >
                    <Icon name="close" size={14} /> Remove from sidebar
                  </button>
                </div>
              )}
            </div>
          ),
        )}
        {creating !== null ? (
          <input
            className="nav-input"
            value={creating}
            autoFocus
            placeholder="Section name"
            onChange={(e) => setCreating(e.currentTarget.value)}
            onBlur={create}
            onKeyDown={(e) => {
              if (e.key === "Enter") create();
              if (e.key === "Escape") setCreating(null);
            }}
          />
        ) : (
          <button className="nav-item nav-add" onClick={() => setCreating("")}>
            <Icon name="plus" size={16} />
            <span className="nav-label">New section</span>
          </button>
        )}

        {suggested.length > 0 && (
          <>
            <button className="nav-heading nav-toggle" onClick={() => setShowSuggested((s) => !s)}>
              <Icon name="chevron" size={12} className={showSuggested ? "open" : ""} /> Suggested
              <span className="nav-count">{suggested.length}</span>
            </button>
            {showSuggested &&
              suggested.map((s) => (
                <div key={s.tag} className="nav-row suggested">
                  <button
                    className="nav-item"
                    title={s.members.join(", ")}
                    onClick={() => onSelect({ view: "section", tag: s.tag, title: s.title })}
                  >
                    <Icon name={iconForTag(s.tag)} size={16} />
                    <span className="nav-label">{s.title}</span>
                    <span className="nav-count">{s.count}</span>
                  </button>
                  <button className="nav-more" aria-label={`Add ${s.title} to sidebar`} onClick={() => pinSection(s.tag, s.title).then(onChanged)}>
                    <Icon name="plus" size={14} />
                  </button>
                </div>
              ))}
          </>
        )}
      </div>

      <div className="nav-group nav-bottom">
        {toTidy > 0 &&
          item({ view: "tidy" }, "Tidy up", "sparkle", <span className="nav-count">{toTidy}</span>)}
        {item({ view: "settings" }, "Settings", "settings")}
      </div>
    </nav>
  );
}

export default Sidebar;
