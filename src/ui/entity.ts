// How pages are labelled and pictured: roles for people, kinds for everything else.
import { ROLES, type Entity } from "../api";
import { dueLabel } from "../format";

const KIND_ICONS: Record<string, string> = {
  Person: "person",
  Project: "project",
  Course: "course",
  Idea: "idea",
  Task: "task",
  Event: "event",
  ResearchArea: "area",
  Document: "document",
  Note: "notes",
  Organization: "building",
};

const TAG_ICONS: Record<string, string> = {
  student: "student",
  alumni: "student",
  collaborator: "people",
  colleague: "people",
  faculty: "building",
  staff: "briefcase",
  person: "people",
  project: "project",
  course: "course",
  idea: "idea",
  task: "task",
  event: "event",
  note: "notes",
  "research-area": "area",
  organization: "building",
};

const ROLE_LABELS: Record<string, string> = {
  student: "Student",
  collaborator: "Collaborator",
  colleague: "Colleague",
  faculty: "Faculty",
  staff: "Staff",
  alumni: "Alumnus",
};

/** Where a project stands, in the order projects are grouped. */
export const PROJECT_STATUSES = [
  { value: "in-progress", label: "In progress" },
  { value: "planned", label: "Planned" },
  { value: "completed", label: "Completed" },
] as const;

export const projectStatusLabel = (status: string | undefined) =>
  PROJECT_STATUSES.find((s) => s.value === status)?.label ?? null;

/** A link's details in a few words: "Associate Professor, since 2019". */
export function detailsText(link: { detail: string | null; since: string | null; until: string | null }) {
  const when =
    link.since && link.until ? `${link.since} to ${link.until}` : link.since ? `since ${link.since}` : link.until ? `until ${link.until}` : null;
  return [link.detail, when].filter(Boolean).join(", ");
}

/** Whether a link with this end date still holds. */
export function isCurrent(until: string | null) {
  if (!until) return true;
  const today = new Date().toISOString().slice(0, 10);
  return today <= until || today.startsWith(until);
}

export const kindLabel = (kind: string) => kind.replace(/([a-z])([A-Z])/g, "$1 $2");

export const iconFor = (entity: Entity) => KIND_ICONS[entity.kind] ?? "hash";

export const iconForTag = (tag: string) => TAG_ICONS[tag] ?? "hash";

/** The person's roles, in the order they are offered. */
export const rolesOf = (entity: Entity) =>
  ROLES.map((r) => r.tag).filter((tag) => entity.tags.includes(tag));

export const roleLabel = (tag: string) => ROLE_LABELS[tag] ?? tag;

/** Tags shown as chips: not roles, following (the Follow button) or favorite (the star). */
export const plainTags = (entity: Entity) =>
  entity.tags.filter((t) => t !== "favorite" && !(entity.kind === "Person" && (t in ROLE_LABELS || t === "following")));

/** What a page is, in a few words: "Student", "Project". */
export function typeLabel(entity: Entity) {
  const roles = entity.kind === "Person" ? rolesOf(entity) : [];
  return roles.length ? roles.map(roleLabel).join(", ") : kindLabel(entity.kind);
}

/** One line identifying a page: "PhD · IIT Guwahati", "Associate Professor · IISc", "Due Fri". */
export function subtitle(entity: Entity) {
  const info = entity.info;
  if (entity.kind === "Person") {
    const what = info.position ?? (info.program ? `${info.program}${rolesOf(entity).includes("student") ? " student" : ""}` : null);
    return [what, info.affiliation].filter(Boolean).join(" · ");
  }
  if (entity.kind === "Organization") return [info.type, info.city].filter(Boolean).join(" · ");
  if (entity.kind === "Course") return [info.code, info.semester].filter(Boolean).join(" · ");
  if (entity.kind === "Task") {
    if (info.status === "done") return "Done";
    return [info.due && `Due ${dueLabel(info.due)}`, info.priority && `${info.priority} priority`, info.status === "waiting" && "waiting"]
      .filter(Boolean)
      .join(" · ");
  }
  if (entity.kind === "Project") return projectStatusLabel(info.status) ?? "";
  return info.status ?? "";
}
