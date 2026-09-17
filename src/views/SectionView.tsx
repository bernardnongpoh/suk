import { useEffect, useState } from "react";
import {
  linkAffiliation,
  listTagged,
  openPage,
  setSectionIcon,
  setTags,
  unlinkedAffiliations,
  type Entity,
  type EntityKind,
  type UnlinkedAffiliation,
} from "../api";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";
import IconPicker from "../ui/IconPicker";
import { iconForTag, PROJECT_STATUSES, projectStatusLabel, subtitle, typeLabel } from "../ui/entity";

interface Props {
  tag: string;
  title: string;
  /** The section's emoji, or empty for its default icon. */
  icon: string;
  version: number;
  onOpen: (id: string) => void;
  onChanged: () => void;
}

/** The kind a section's own tag stands for, so new pages there get that kind. */
const KIND_FOR_TAG: Record<string, EntityKind> = {
  person: "Person", project: "Project", course: "Course", idea: "Idea", organization: "Organization",
};
const ROLE_TAGS = ["student", "collaborator", "colleague", "faculty", "staff", "alumni"];

/** Everything with a tag, e.g. all students, or everything tagged #nba-committee. */
function SectionView({ tag, title, icon, version, onOpen, onChanged }: Props) {
  const [entities, setEntities] = useState<Entity[] | null>(null);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState<string | null>(null);
  // On Organizations: people whose affiliation is still only text, to turn into links.
  const [unlinked, setUnlinked] = useState<UnlinkedAffiliation[]>([]);

  useEffect(() => {
    if (tag !== "organization") return;
    unlinkedAffiliations().then(setUnlinked, () => setUnlinked([]));
  }, [tag, version]);

  async function linkOne(item: UnlinkedAffiliation) {
    try {
      await linkAffiliation(item.person.id, item.organization?.name ?? item.text);
      onChanged();
    } catch (err) {
      setError(String(err));
    }
  }

  useEffect(() => {
    let cancelled = false;
    listTagged(tag).then(
      (result) => !cancelled && setEntities(result),
      (err) => !cancelled && setError(String(err)),
    );
    return () => {
      cancelled = true;
    };
  }, [tag, version]);

  async function add() {
    const name = adding?.trim();
    setAdding(null);
    if (!name) return;
    try {
      // In a role section ("Students") a new page is a person with that role.
      const kind = KIND_FOR_TAG[tag] ?? (ROLE_TAGS.includes(tag) ? "Person" : undefined);
      const page = await openPage(name, kind);
      if (!KIND_FOR_TAG[tag] && !page.tags.includes(tag)) await setTags(page.id, [...page.tags, tag]);
      onChanged();
      onOpen(page.id);
    } catch (err) {
      setError(String(err));
    }
  }

  const list = (items: Entity[], quiet = false) => (
    <ul className={`list${quiet ? " quiet" : ""}`}>
      {items.map((e) => (
        <li key={e.id} className="row clickable" onClick={() => onOpen(e.id)}>
          <Avatar entity={e} size={34} />
          <span className="row-main">
            <span className="row-title">
              {e.name}
              {e.info.full_name && e.info.full_name !== e.name && <span className="row-aside"> {e.info.full_name}</span>}
            </span>
            <span className="row-sub">
              {(tag === "project" ? [e.info.funding, e.notes.split("\n")[0]] : [typeLabel(e), subtitle(e)]).filter(Boolean).join(" · ")}
            </span>
          </span>
          {e.info.email && <span className="row-meta">{e.info.email}</span>}
        </li>
      ))}
    </ul>
  );

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-header-title">
          <span className="menu-anchor">
            <button className={`page-icon${icon ? " emoji" : ""}`} title="Choose icon" aria-label="Choose icon" onClick={() => setPicking((p) => !p)}>
              {icon || <Icon name={iconForTag(tag)} size={20} />}
            </button>
            {picking && (
              <IconPicker
                page={{ kind: "Section", name: title, tags: [tag] }}
                current={icon || null}
                onPick={(chosen) => setSectionIcon(tag, chosen).then(onChanged, (err) => setError(String(err)))}
                onClose={() => setPicking(false)}
              />
            )}
          </span>
          <div>
            <h1>{title}</h1>
            <p className="muted">{entities ? `${entities.length} ${entities.length === 1 ? "page" : "pages"}` : ""}</p>
          </div>
        </div>
        {adding === null && (
          <button className="button" onClick={() => setAdding("")}>
            <Icon name="plus" size={14} />
            New
          </button>
        )}
      </div>
      {unlinked.length > 0 && (
        <div className="org-banner">
          <div className="card-title">
            {unlinked.length === 1 ? "1 person's affiliation is" : `${unlinked.length} people's affiliations are`} still only text
          </div>
          <div className="card-sub">Link them so they show up on the organization's page.</div>
          <ul>
            {unlinked.map((item) => (
              <li key={item.person.id}>
                <Avatar entity={item.person} size={24} />
                <span className="row-main">
                  <span className="row-title">{item.person.name}</span> <span className="muted">"{item.text}"</span>
                </span>
                <button className="button small" onClick={() => linkOne(item)}>
                  <Icon name="link" size={12} />
                  {item.organization ? `Link to ${item.organization.name}` : `Create "${item.text}"`}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {adding !== null && (
        <input
          className="new-page-input"
          value={adding}
          autoFocus
          placeholder="Name, then Enter"
          onChange={(e) => setAdding(e.currentTarget.value)}
          onBlur={add}
          onKeyDown={(e) => {
            if (e.key === "Enter") add();
            if (e.key === "Escape") setAdding(null);
          }}
        />
      )}
      {error ? (
        <p className="form-error">{error}</p>
      ) : entities === null ? null : entities.length === 0 ? (
        <p className="empty">
          Nothing here yet. Tag a page #{tag}, or mention one in the chat.
        </p>
      ) : tag === "project" ? (
        // Projects by where they stand.
        [...PROJECT_STATUSES.map((s) => s.value), undefined].map((status) => {
          const group = entities.filter((e) => (projectStatusLabel(e.info.status) ? e.info.status : undefined) === status);
          if (!group.length) return null;
          return (
            <section key={status ?? "none"}>
              <h2 className="section-title">
                <span className={`status-pill-dot ${status ?? "none"}`} />
                {projectStatusLabel(status) ?? "No status"} <span className="count">{group.length}</span>
              </h2>
              {list(group, status === "completed")}
            </section>
          );
        })
      ) : (
        list(entities)
      )}
    </div>
  );
}

export default SectionView;
