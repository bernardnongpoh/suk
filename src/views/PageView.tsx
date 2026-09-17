import { useEffect, useState } from "react";
import {
  deletePage,
  getEntity,
  assignTask,
  linkAffiliation,
  listEntities,
  openInObsidian,
  openPage,
  openUrl,
  renamePage,
  ROLES,
  setAliases,
  setFavorite,
  setIcon,
  setTags,
  setTaskDone,
  submitDetails,
  type Affiliation,
  type Entity,
  type EntityDetail,
  type Link,
  PROFILE_LINKS,
} from "../api";
import ChatView from "../chat/ChatView";
import { dueLabel, keyLabel } from "../format";
import NotesEditor from "../notes/NotesEditor";
import TaskList from "../tasks/TaskList";
import FollowPanel from "../people/FollowPanel";
import ProfileForm from "../people/ProfileForm";
import type { ChatMessage } from "../types";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";
import IconPicker from "../ui/IconPicker";
import {
  detailsText,
  iconForTag,
  isCurrent,
  kindLabel,
  plainTags,
  PROJECT_STATUSES,
  projectStatusLabel,
  roleLabel,
  rolesOf,
  subtitle,
  typeLabel,
} from "../ui/entity";

interface Props {
  id: string;
  /** Bumped when pages may have changed elsewhere (chat, Obsidian). */
  version: number;
  onOpen: (id: string) => void;
  onBack: (() => void) | null;
  onChanged: () => void;
}

/** Details shown elsewhere on the page, or internal to the app. */
const HIDDEN_INFO = ["details_skipped", "title", "full_name", "icon"];

/** Icons and order for well-known details; others follow alphabetically. */
const PROPS: Record<string, string> = {
  email: "mail",
  position: "briefcase",
  affiliation: "building",
  department: "building",
  homepage: "globe",
  program: "student",
  start: "calendar",
  thesis: "notes",
  due: "calendar",
  priority: "flag",
  status: "check",
  type: "building",
  city: "globe",
  country: "globe",
};

/** Section heading for a relationship, as seen from the page being viewed. */
function section(link: Link): string {
  if (link.kind === "LINKS_TO") return link.outgoing ? "Links to" : "Mentioned in";
  if (link.kind === "SUPERVISES") return link.outgoing ? "Supervises" : "Supervised by";
  if (link.kind === "COLLABORATES_WITH") return link.outgoing ? "Collaborators" : "Collaborates on";
  if (link.kind === "WORKS_ON") return link.outgoing ? "Works on" : "People";
  if (link.kind === "TAKES") return link.outgoing ? "Courses" : "Students";
  if (link.kind === "SCHEDULED_FOR") return link.outgoing ? "For task" : "Scheduled";
  if (link.kind === "ASSIGNED_TO") return "Assigned to";
  if (link.kind === "FOR") return "For";
  if (link.kind === "WAITING_ON") return "Waiting on";
  if (link.kind === "HAS_TASK" && !link.outgoing) return "Part of";
  if (link.kind === "PART_OF") return link.outgoing ? "Part of" : "Departments and groups";
  const plural: Record<string, string> = {
    Task: "Tasks", Idea: "Ideas", Project: "Projects", Course: "Courses", Person: "People",
    ResearchArea: "Related", Document: "Documents", Event: "Events", Note: "Notes",
  };
  return plural[link.other.kind] ?? "Related";
}

function PageView({ id, version, onOpen, onBack, onChanged }: Props) {
  const [detail, setDetail] = useState<EntityDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [chat, setChat] = useState<ChatMessage[]>([]);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [tagDraft, setTagDraft] = useState<string | null>(null);
  const [menu, setMenu] = useState<"page" | "role" | "assign" | "status" | "icon" | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [hideProfile, setHideProfile] = useState(false);
  const [showProfile, setShowProfile] = useState(false);
  const [aliasDraft, setAliasDraft] = useState<string | null>(null);
  // Assigning a task: the people to pick from, filtered by what's typed.
  const [people, setPeople] = useState<Entity[] | null>(null);
  const [assignQuery, setAssignQuery] = useState("");

  const load = () => getEntity(id).then(setDetail, (err) => setError(String(err)));
  useEffect(() => {
    load();
  }, [id, version]);

  if (!detail) {
    return error ? (
      <div className="page">
        <p className="form-error">{error}</p>
      </div>
    ) : null;
  }
  const { entity, links, details, affiliations, unlinked_affiliation: unlinked } = detail;
  const person = entity.kind === "Person";

  const changed = () => {
    load();
    onChanged();
  };
  const follow = (name: string) => openPage(name).then((page) => { onChanged(); onOpen(page.id); });
  const report = (p: Promise<unknown>) => p.catch((err) => setError(String(err)));
  const updateTags = (tags: string[]) => report(setTags(entity.id, tags).then(changed));

  async function rename() {
    const name = renaming?.trim();
    setRenaming(null);
    if (!name || name === entity.name) return;
    await report(renamePage(entity.id, name));
    changed();
  }

  function addTag() {
    const tag = tagDraft?.trim();
    setTagDraft(null);
    if (tag) updateTags([...entity.tags, tag]);
  }

  const task = entity.kind === "Task";
  const done = entity.info.status === "done";
  // A page's tasks are listed with checkboxes below, not as plain connections.
  const isTaskLink = (l: Link) =>
    (!task && l.other.kind === "Task" && ["FOR", "WAITING_ON", "HAS_TASK", "ASSIGNED_TO"].includes(l.kind)) ||
    (task && l.kind === "ASSIGNED_TO");
  const assignees = task ? links.filter((l) => l.kind === "ASSIGNED_TO" && l.outgoing).map((l) => l.other) : [];
  const hasTasks = !task && links.some(isTaskLink);
  const organization = entity.kind === "Organization";
  const project = entity.kind === "Project";
  const favorite = entity.tags.includes("favorite");
  // Where a person works or studied, and who is at an organization, get sections of their own.
  const isAffiliationLink = (l: Link) =>
    ["AFFILIATED_WITH", "STUDIED_AT"].includes(l.kind) && (person ? l.outgoing : organization && !l.outgoing);
  const currentWork = person ? links.filter((l) => l.outgoing && l.kind === "AFFILIATED_WITH" && isCurrent(l.until)) : [];
  const background = person ? links.filter((l) => isAffiliationLink(l) && !currentWork.includes(l)) : [];
  const groups = new Map<string, Link[]>();
  for (const link of links.filter((l) => !isTaskLink(l) && !isAffiliationLink(l))) {
    const name = section(link);
    groups.set(name, [...(groups.get(name) ?? []), link]);
  }
  const order = Object.keys(PROPS);
  const rank = (k: string) => (order.includes(k) ? order.indexOf(k) : order.length);
  const info = Object.entries(entity.info)
    .filter(([k]) => !HIDDEN_INFO.includes(k) && !(person && (k === "affiliation" || PROFILE_LINKS.some((l) => l.key === k))))
    // A project's status is chosen in the header.
    .filter(([k]) => !(project && k === "status"))
    .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b));
  const roles = rolesOf(entity);
  const fullName = entity.info.full_name && entity.info.full_name !== entity.name ? entity.info.full_name : null;
  // A project's status has its own control in the header.
  const sub = [fullName, project ? entity.info.funding : subtitle(entity)].filter(Boolean).join(" · ");

  async function updateAliases(aliases: string[]) {
    await report(setAliases(entity.id, aliases).then(changed));
  }
  function addAlias() {
    const alias = aliasDraft?.trim();
    setAliasDraft(null);
    if (alias) updateAliases([...entity.aliases, alias]);
  }

  // People at an organization: now, then before; each with where inside it and what they do.
  const affiliationCard = (a: Affiliation) => {
    const where = a.organization.id !== entity.id ? `in ${a.organization.name}` : null;
    const what = [a.kind === "STUDIED_AT" ? `studied${a.detail ? ` (${a.detail})` : ""}` : a.detail, where]
      .filter(Boolean)
      .join(" ");
    const when = detailsText({ detail: null, since: a.since, until: a.until });
    return (
      <button key={`${a.person.id}-${a.kind}-${a.organization.id}`} className="link-card" onClick={() => onOpen(a.person.id)}>
        <Avatar entity={a.person} size={30} />
        <span className="link-card-text">
          <span className="link-card-name">{a.person.name}</span>
          <span className="link-card-sub">{[what || typeLabel(a.person), when].filter(Boolean).join(" · ")}</span>
        </span>
      </button>
    );
  };
  const nowAt = affiliations.filter((a) => a.kind === "AFFILIATED_WITH" && isCurrent(a.until));
  const before = affiliations.filter((a) => !nowAt.includes(a));

  return (
    <div className="page-split">
      <div className="page page-main">
        <div className="page-toolbar">
          {onBack ? (
            <button className="button ghost small" onClick={onBack}>
              <Icon name="back" size={14} />
              Back
            </button>
          ) : (
            <span />
          )}
          <div className="page-actions">
            <button
              className={`icon-button favorite${favorite ? " on" : ""}`}
              aria-label={favorite ? "Remove from favorites" : "Add to favorites"}
              aria-pressed={favorite}
              title={favorite ? "Remove from favorites" : "Add to favorites"}
              onClick={() => report(setFavorite(entity.id, !favorite).then(changed))}
            >
              <Icon name="star" size={16} />
            </button>
            <button className="button ghost small" onClick={() => report(openInObsidian(entity.id))}>
              <Icon name="obsidian" size={14} />
              Open in Obsidian
            </button>
            <div className="menu-anchor">
              <button className="icon-button" aria-label="Page options" onClick={() => setMenu(menu === "page" ? null : "page")}>
                <Icon name="more" size={16} />
              </button>
              {menu === "page" && (
                <div className="menu right" onMouseLeave={() => { setMenu(null); setConfirmDelete(false); }}>
                  <button onClick={() => { setMenu(null); setRenaming(entity.name); }}>
                    <Icon name="pencil" size={14} /> Rename
                  </button>
                  <button
                    className="danger"
                    onClick={() =>
                      confirmDelete
                        ? deletePage(entity.id).then(() => { onChanged(); onBack?.(); }, (err) => setError(String(err)))
                        : setConfirmDelete(true)
                    }
                  >
                    <Icon name="trash" size={14} /> {confirmDelete ? `Delete ${entity.name}?` : "Delete…"}
                  </button>
                </div>
              )}
            </div>
          </div>
        </div>

        {error && <p className="form-error">{error}</p>}

        <header className="page-hero">
          <span className="menu-anchor">
            <button
              className={`hero-icon${entity.info.icon ? " has-icon" : ""}`}
              title={entity.info.icon ? "Change icon" : "Add an icon"}
              aria-label={entity.info.icon ? "Change icon" : "Add an icon"}
              onClick={() => setMenu(menu === "icon" ? null : "icon")}
            >
              <Avatar entity={entity} size={person ? 64 : 56} />
              <span className="hero-icon-hint">
                <Icon name="smile" size={13} />
              </span>
            </button>
            {menu === "icon" && (
              <IconPicker
                page={entity}
                current={entity.info.icon ?? null}
                onPick={(icon) => report(setIcon(entity.id, icon).then(changed))}
                onClose={() => setMenu(null)}
              />
            )}
          </span>
          <div className="page-hero-text">
            {renaming !== null ? (
              <input
                className="title-input"
                value={renaming}
                autoFocus
                onChange={(e) => setRenaming(e.currentTarget.value)}
                onBlur={rename}
                onKeyDown={(e) => {
                  if (e.key === "Enter") rename();
                  if (e.key === "Escape") setRenaming(null);
                }}
              />
            ) : (
              <h1 className={`page-title${task && done ? " done" : ""}`} title="Double-click to rename" onDoubleClick={() => setRenaming(entity.name)}>
                {task && (
                  <button
                    className="task-check large"
                    role="checkbox"
                    aria-checked={done}
                    aria-label={done ? "Reopen task" : "Mark done"}
                    onClick={() => report(setTaskDone(entity.id, !done).then(changed))}
                  >
                    <Icon name="check" size={14} />
                  </button>
                )}
                {entity.name}
              </h1>
            )}
            <p className="page-sub">{sub || typeLabel(entity)}</p>
            <div className="chips">
              {person ? (
                roles.map((role) => (
                  <span key={role} className="chip role">
                    <Icon name={iconForTag(role)} size={13} />
                    {roleLabel(role)}
                    <button aria-label={`Remove ${roleLabel(role)}`} onClick={() => updateTags(entity.tags.filter((t) => t !== role))}>
                      <Icon name="close" size={11} />
                    </button>
                  </span>
                ))
              ) : (
                <span className="chip role">
                  <Icon name={iconForTag(entity.kind.toLowerCase())} size={13} />
                  {kindLabel(entity.kind)}
                </span>
              )}
              {project && (
                <span className="menu-anchor">
                  <button
                    className={`chip status-pill ${entity.info.status ?? "none"}`}
                    aria-haspopup="menu"
                    onClick={() => setMenu(menu === "status" ? null : "status")}
                  >
                    <span className="status-mark" />
                    {projectStatusLabel(entity.info.status) ?? "Set status"}
                    <Icon name="chevron" size={11} className="open" />
                  </button>
                  {menu === "status" && (
                    <div className="menu" role="menu" onMouseLeave={() => setMenu(null)}>
                      {PROJECT_STATUSES.map((s) => (
                        <button
                          key={s.value}
                          role="menuitemradio"
                          aria-checked={entity.info.status === s.value}
                          onClick={() => {
                            setMenu(null);
                            if (s.value !== entity.info.status) report(submitDetails(entity.id, { status: s.value }, []).then(changed));
                          }}
                        >
                          <span className={`status-pill-dot ${s.value}`} /> {s.label}
                          {entity.info.status === s.value && <Icon name="check" size={13} className="menu-check" />}
                        </button>
                      ))}
                    </div>
                  )}
                </span>
              )}
              {task &&
                assignees.map((p) => (
                  <span key={p.id} className="chip assignee">
                    <span className="task-chip-label">Assigned to</span>
                    <button className="assignee-name" onClick={() => onOpen(p.id)}>
                      <Avatar entity={p} size={16} />
                      {p.name}
                    </button>
                    <button aria-label={`Unassign ${p.name}`} onClick={() => report(assignTask(entity.id, p.id, false).then(changed))}>
                      <Icon name="close" size={11} />
                    </button>
                  </span>
                ))}
              {task && (
                <span className="menu-anchor">
                  <button
                    className="chip add"
                    onClick={() => {
                      setMenu(menu === "assign" ? null : "assign");
                      setAssignQuery("");
                      listEntities("Person").then(setPeople, (err) => setError(String(err)));
                    }}
                  >
                    <Icon name="plus" size={12} /> Assign
                  </button>
                  {menu === "assign" && (
                    <div className="menu assign-menu" onMouseLeave={() => setMenu(null)}>
                      <input
                        className="menu-search"
                        autoFocus
                        placeholder="Who is doing this?"
                        value={assignQuery}
                        onChange={(e) => setAssignQuery(e.currentTarget.value)}
                      />
                      {(people ?? [])
                        .filter((p) => !assignees.some((a) => a.id === p.id))
                        .filter((p) => [p.name, p.info.full_name ?? ""].some((n) => n.toLowerCase().includes(assignQuery.toLowerCase())))
                        .slice(0, 8)
                        .map((p) => (
                          <button
                            key={p.id}
                            onClick={() => {
                              setMenu(null);
                              report(assignTask(entity.id, p.id, true).then(changed));
                            }}
                          >
                            <Avatar entity={p} size={18} /> {p.name}
                            <span className="muted small">{typeLabel(p)}</span>
                          </button>
                        ))}
                      {people && people.length === 0 && <span className="muted small menu-note">No people yet.</span>}
                    </div>
                  )}
                </span>
              )}
              {person && (
                <span className="menu-anchor">
                  <button className="chip add" onClick={() => setMenu(menu === "role" ? null : "role")}>
                    <Icon name="plus" size={12} /> Role
                  </button>
                  {menu === "role" && (
                    <div className="menu" onMouseLeave={() => setMenu(null)}>
                      {ROLES.filter((r) => !roles.includes(r.tag)).map((r) => (
                        <button key={r.tag} onClick={() => { setMenu(null); updateTags([...entity.tags, r.tag]); }}>
                          <Icon name={iconForTag(r.tag)} size={14} /> {r.label}
                        </button>
                      ))}
                    </div>
                  )}
                </span>
              )}
              {plainTags(entity).map((tag) => (
                <span key={tag} className="chip">
                  #{tag}
                  <button aria-label={`Remove tag ${tag}`} onClick={() => updateTags(entity.tags.filter((t) => t !== tag))}>
                    <Icon name="close" size={11} />
                  </button>
                </span>
              ))}
              {tagDraft !== null ? (
                <input
                  className="chip-input"
                  value={tagDraft}
                  autoFocus
                  placeholder="tag"
                  onChange={(e) => setTagDraft(e.currentTarget.value)}
                  onBlur={addTag}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") addTag();
                    if (e.key === "Escape") setTagDraft(null);
                  }}
                />
              ) : (
                <button className="chip add" onClick={() => setTagDraft("")}>
                  <Icon name="plus" size={12} /> Tag
                </button>
              )}
            </div>
            {(organization || entity.aliases.length > 0) && (
              <div className="chips aliases">
                <span className="muted small">Also called</span>
                {entity.aliases.map((alias) => (
                  <span key={alias} className="chip">
                    {alias}
                    <button aria-label={`Remove ${alias}`} onClick={() => updateAliases(entity.aliases.filter((a) => a !== alias))}>
                      <Icon name="close" size={11} />
                    </button>
                  </span>
                ))}
                {aliasDraft !== null ? (
                  <input
                    className="chip-input"
                    value={aliasDraft}
                    autoFocus
                    placeholder="IITG"
                    onChange={(e) => setAliasDraft(e.currentTarget.value)}
                    onBlur={addAlias}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") addAlias();
                      if (e.key === "Escape") setAliasDraft(null);
                    }}
                  />
                ) : (
                  <button className="chip add" onClick={() => setAliasDraft("")}>
                    <Icon name="plus" size={12} /> Name
                  </button>
                )}
              </div>
            )}
          </div>
        </header>

        {details && !hideProfile && (details.roles.length > 0 || showProfile) ? (
          // Someone whose connection is unknown gets the whole form; known people a quiet link.
          <ProfileForm
            key={`${entity.id}-${entity.updated_at}`}
            request={details}
            place="page"
            onChange={changed}
            onClose={() => (showProfile ? setShowProfile(false) : setHideProfile(true))}
          />
        ) : (
          details && (
            <button className="text-button add-details" onClick={() => setShowProfile(true)}>
              <Icon name="plus" size={13} />
              Add {[...details.fields, ...roles.flatMap((r) => details.role_fields[r] ?? [])].map((f) => f.label.toLowerCase()).join(", ")}
            </button>
          )
        )}

        {(info.length > 0 || currentWork.length > 0 || unlinked) && (
          <dl className="props">
            {currentWork.length > 0 && (
              <div className="prop">
                <dt>
                  <Icon name="building" size={14} />
                  Affiliation
                </dt>
                <dd className="chips">
                  {currentWork.map((l) => (
                    <button key={l.other.id} className="task-chip org-chip" onClick={() => onOpen(l.other.id)}>
                      <Avatar entity={l.other} size={16} />
                      {l.other.name}
                      {detailsText(l) && <span className="task-chip-label">{detailsText(l)}</span>}
                    </button>
                  ))}
                </dd>
              </div>
            )}
            {unlinked && (
              <div className="prop">
                <dt>
                  <Icon name="building" size={14} />
                  Affiliation
                </dt>
                <dd className="unlinked">
                  <span>{unlinked[0]}</span>
                  <button className="button small" onClick={() => report(linkAffiliation(entity.id, unlinked[1]?.name ?? unlinked[0]).then(changed))}>
                    <Icon name="link" size={12} />
                    {unlinked[1] ? `Link to ${unlinked[1].name}` : "Make it a page"}
                  </button>
                </dd>
              </div>
            )}
            {info.map(([key, value]) => (
              <div key={key} className="prop">
                <dt>
                  <Icon name={PROPS[key] ?? "hash"} size={14} />
                  {keyLabel(key)}
                </dt>
                <dd>
                  {key === "homepage" && /^https?:\/\//.test(value) ? (
                    <button className="text-button" onClick={() => report(openUrl(value))}>
                      {value.replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, "")}
                      <Icon name="external" size={12} />
                    </button>
                  ) : key === "due" ? (
                    `${dueLabel(value)} (${value})`
                  ) : (
                    value
                  )}
                </dd>
              </div>
            ))}
          </dl>
        )}

        {person && <FollowPanel person={entity} version={version} onOpen={onOpen} onChanged={changed} />}

        <section className="page-section">
          <h2 className="section-title">Notes</h2>
          <NotesEditor page={entity} onLink={follow} onSaved={changed} />
        </section>

        {hasTasks && (
          <section className="page-section">
            <h2 className="section-title">Open tasks</h2>
            <TaskList
              about={entity.id}
              version={version}
              onOpen={onOpen}
              empty={<p className="muted">All done.</p>}
            />
          </section>
        )}

        {organization && nowAt.length > 0 && (
          <section className="page-section">
            <h2 className="section-title">
              People <span className="count">{nowAt.length}</span>
            </h2>
            <div className="link-grid">{nowAt.map(affiliationCard)}</div>
          </section>
        )}
        {organization && before.length > 0 && (
          <section className="page-section">
            <h2 className="section-title">
              Studied or worked here before <span className="count">{before.length}</span>
            </h2>
            <div className="link-grid">{before.map(affiliationCard)}</div>
          </section>
        )}
        {background.length > 0 && (
          <section className="page-section">
            <h2 className="section-title">Background</h2>
            <div className="link-grid">
              {background.map((l) => (
                <button key={l.kind + l.other.id} className="link-card" onClick={() => onOpen(l.other.id)}>
                  <Avatar entity={l.other} size={30} />
                  <span className="link-card-text">
                    <span className="link-card-name">{l.other.name}</span>
                    <span className="link-card-sub">
                      {[l.kind === "STUDIED_AT" ? "Studied" : "Worked", detailsText(l)].filter(Boolean).join(" · ")}
                    </span>
                  </span>
                </button>
              ))}
            </div>
          </section>
        )}

        {[...groups].map(([name, group]) => (
          <section key={name} className="page-section">
            <h2 className="section-title">
              {name} <span className="count">{group.length}</span>
            </h2>
            <div className="link-grid">
              {group.map((l) => (
                <button key={l.kind + l.other.id + l.outgoing} className="link-card" onClick={() => onOpen(l.other.id)}>
                  <Avatar entity={l.other} size={30} />
                  <span className="link-card-text">
                    <span className="link-card-name">{l.other.name}</span>
                    <span className="link-card-sub">
                      {l.other.info.status === "done" ? "Done" : detailsText(l) || subtitle(l.other) || typeLabel(l.other)}
                    </span>
                  </span>
                </button>
              ))}
            </div>
          </section>
        ))}
      </div>

      <aside className="page-chat">
        <ChatView key={entity.id} messages={chat} setMessages={setChat} focus={entity} onChanged={changed} onOpen={onOpen} />
      </aside>
    </div>
  );
}

export default PageView;
