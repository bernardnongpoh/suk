// Typed wrappers for the backend commands in src-tauri/src/commands.rs.
import { Channel, invoke } from "@tauri-apps/api/core";

export interface Entity {
  id: string;
  kind: string;
  name: string;
  /** Details such as email, program, due, priority. */
  info: Record<string, string>;
  /** Tags beyond the one the kind implies. */
  tags: string[];
  /** Markdown the user writes on the page. */
  notes: string;
  updated_at: number;
  /** Other names the page is found by ("IITG"). */
  aliases: string[];
}

export interface Link {
  kind: string;
  /** True when the entity being viewed is the source. */
  outgoing: boolean;
  other: Entity;
  /** A position or degree. */
  detail: string | null;
  since: string | null;
  until: string | null;
}

/** Someone's link to an organization, as seen from that organization. */
export interface Affiliation {
  person: Entity;
  kind: "AFFILIATED_WITH" | "STUDIED_AT";
  /** The organization itself, or the department or lab inside it. */
  organization: Entity;
  detail: string | null;
  since: string | null;
  until: string | null;
}

/** How a detail is edited: "due" is a date with an optional time; "choice" allows only its options. */
export type DetailInput = "text" | "email" | "url" | "due" | "datetime" | "choice";

/** A detail a kind of page usually has. */
export interface DetailField {
  key: string;
  label: string;
  input: DetailInput;
  /** Allowed values for "choice", suggestions otherwise. */
  options: string[];
}

export interface EntityDetail {
  entity: Entity;
  links: Link[];
  /** For a person, what is still unknown about them. */
  details: DetailsRequest | null;
  /** For an organization, everyone there or in its departments, past and present. */
  affiliations: Affiliation[];
  /** For a person whose affiliation is only text: the text, and the organization it names. */
  unlinked_affiliation: [string, Entity | null] | null;
  /** The details this kind of page usually has, in order. */
  fields: DetailField[];
}

export type EntityKind =
  | "Person"
  | "Project"
  | "Course"
  | "Idea"
  | "Task"
  | "Event"
  | "Note"
  | "Organization";

export interface ChatRecord {
  id: string;
  at: number;
  role: "user" | "agent";
  text: string;
  focus: string | null;
}

export interface Hit {
  entity: Entity;
  /** "name", an info key such as "email", "tag" or "notes". */
  field: string;
  snippet: string | null;
}

export interface SectionIdea {
  tag: string;
  title: string;
  members: string[];
  count: number;
}

export interface Field {
  key: string;
  label: string;
  options: string[];
}

export interface RoleOption {
  tag: string;
  label: string;
}

/** What to ask about a person. */
export interface DetailsRequest {
  entity: Entity;
  /** Connections to choose from; empty when the person already has a role. */
  roles: RoleOption[];
  /** Missing identity details. */
  fields: Field[];
  /** Missing details for each role, shown once it is chosen. */
  role_fields: Record<string, Field[]>;
}

/** Roles a person can have, as tags. */
export const ROLES: RoleOption[] = [
  { tag: "student", label: "My student" },
  { tag: "collaborator", label: "Collaborator" },
  { tag: "colleague", label: "Colleague" },
  { tag: "faculty", label: "Faculty elsewhere" },
  { tag: "staff", label: "Staff" },
  { tag: "alumni", label: "Former student" },
];

/** A task with who it's for, who the user is waiting on, and its project or course. */
export interface TaskItem {
  task: Entity;
  /** Who is to do it; empty for the user's own tasks. */
  assigned_to: Entity[];
  for: Entity[];
  waiting_on: Entity[];
  part_of: Entity[];
}

export interface Sidebar {
  /** Starred pages, by name. */
  favorites: Entity[];
  /** `icon` is an emoji, or empty for the default. */
  sections: { tag: string; title: string; count: number; icon: string }[];
  suggested: SectionIdea[];
}

export interface VaultInfo {
  path: string;
  registered: boolean;
}

export interface ScheduleItem {
  title: string;
  /** Local time, "YYYY-MM-DDTHH:MM". */
  start: string;
  end: string;
  task?: string;
  notes?: string;
}

export type ItemState = "proposed" | "confirmed" | "added" | "skipped";

export interface Proposal {
  id: string;
  summary: string | null;
  items: ScheduleItem[];
  states: ItemState[];
  status: "pending" | "confirmed" | "dismissed";
}

export interface ChatReply {
  text: string;
  proposals: Proposal[];
  notice: string | null;
  sections: SectionIdea[];
  details: DetailsRequest[];
}

export interface ChatStatus {
  status: string;
}

export type AssistantKind = "claude" | "codex";

/** Where an assistant stands on this computer. */
export interface AssistantInfo {
  kind: AssistantKind;
  label: string;
  installed: boolean;
  version: string | null;
  signed_in: boolean;
  account: string | null;
  /** The official install command, for a terminal. */
  install_command: string;
  /** The plan someone needs, e.g. "a Claude Pro or Max plan…". */
  requirement: string;
}

export interface AssistantStatus {
  chosen: AssistantKind | null;
  /** The chosen assistant is installed and signed in. */
  ready: boolean;
  /** Claude Code, then Codex. */
  assistants: AssistantInfo[];
  /** Google Calendar connector status, known once Claude Code has answered a message. */
  calendar_status: string | null;
  calendar_help: string;
}

/** What to show while signing in. */
export interface SignInPrompt {
  url: string | null;
  /** Codex: the code to enter on the sign-in page. */
  code: string | null;
  /** Claude Code: the page shows a code to paste back. */
  needs_code: boolean;
}

function statusChannel(onStatus: (status: string) => void) {
  const channel = new Channel<ChatStatus>();
  channel.onmessage = (m) => onStatus(m.status);
  return channel;
}

/**
 * `focus` is the id of the page the message is sent from; `mentions` are ids of pages picked
 * while typing.
 */
export const sendMessage = (
  message: string,
  focus: string | null,
  mentions: string[],
  onStatus: (status: string) => void,
) =>
  invoke<ChatReply>("send_message", { message, focus, mentions, onStatus: statusChannel(onStatus) });

/** Claude reads a profile link and fills in the person's details. */
export const fillProfile = (id: string, url: string, onStatus: (status: string) => void) =>
  invoke<{ entity: Entity; text: string }>("fill_profile", { id, url, onStatus: statusChannel(onStatus) });

export const confirmProposal = (
  id: string,
  items: number[],
  onStatus: (status: string) => void,
) =>
  invoke<ChatReply>("confirm_proposal", { id, items, onStatus: statusChannel(onStatus) });

export const dismissProposal = (id: string) => invoke<Proposal>("dismiss_proposal", { id });

export const chatHistory = (focus: string | null) =>
  invoke<ChatRecord[]>("chat_history", { focus });

export const search = (query: string) => invoke<Hit[]>("search", { query });

export interface UnlinkedAffiliation {
  person: Entity;
  text: string;
  organization: Entity | null;
}

/** People whose affiliation is still only text. */
export const unlinkedAffiliations = () => invoke<UnlinkedAffiliation[]>("unlinked_affiliations");

/** Links a person to an organization page by any of its names, creating it if needed. */
export const linkAffiliation = (person: string, organization: string) =>
  invoke<Entity>("link_affiliation", { person, organization });

export const setAliases = (id: string, aliases: string[]) => invoke<Entity>("set_aliases", { id, aliases });

/** Tasks from the database, soonest first; `about` limits them to one person, project or course. */
export const listTasks = (status: "open" | "done" | "all", about?: string, assigned?: "mine" | "others") =>
  invoke<TaskItem[]>("list_tasks", { status, about: about ?? null, assigned: assigned ?? null });

/** Assigns a task to a person, or takes it off them. */
export const assignTask = (task: string, person: string, assigned: boolean) =>
  invoke<void>("assign_task", { task, person, assigned });

export const setTaskDone = (id: string, done: boolean) => invoke<Entity>("set_task_done", { id, done });

export const getSidebar = () => invoke<Sidebar>("get_sidebar");

export const pinSection = (tag: string, title: string) =>
  invoke<void>("pin_section", { tag, title });

export const dismissSection = (tag: string) => invoke<void>("dismiss_section", { tag });

export const listTagged = (tag: string) => invoke<Entity[]>("list_tagged", { tag });

export const submitDetails = (id: string, values: Record<string, string>, roles: string[]) =>
  invoke<Entity>("submit_details", { id, values, roles });

/** Opens a web link in the browser. */
export const openUrl = (url: string) => invoke<void>("open_url", { url });

export const skipDetails = (id: string) => invoke<Entity>("skip_details", { id });

export const saveNotes = (id: string, notes: string) =>
  invoke<Entity>("save_notes", { id, notes });

export const setTags = (id: string, tags: string[]) => invoke<Entity>("set_tags", { id, tags });

/** The page with this name; a new one (a note, unless `kind` is given) if there is none. */
export const openPage = (name: string, kind?: EntityKind) =>
  invoke<Entity>("open_page", { name, kind: kind ?? null });

export const renamePage = (id: string, name: string) =>
  invoke<Entity>("rename_page", { id, name });

export const deletePage = (id: string) => invoke<void>("delete_page", { id });

export const getVault = () => invoke<VaultInfo>("get_vault");

/** Opens a page's Markdown file, or the folder with null: in Obsidian if it's a vault there, otherwise in Finder or the file manager. */
export const openPageFile = (id: string | null) => invoke<void>("open_page_file", { id });

export const listEntities = (kind: EntityKind) =>
  invoke<Entity[]>("list_entities", { kind });

export const getEntity = (id: string) => invoke<EntityDetail>("get_entity", { id });

export const assistantStatus = () => invoke<AssistantStatus>("assistant_status");

export const chooseAssistant = (kind: AssistantKind) => invoke<void>("choose_assistant", { kind });

/** Runs the official installer; each line of its output goes to `onLine`. */
export function installAssistant(kind: AssistantKind, onLine: (line: string) => void) {
  const channel = new Channel<{ line: string }>();
  channel.onmessage = (m) => onLine(m.line);
  return invoke<AssistantInfo>("install_assistant", { kind, onOutput: channel });
}

/** Signs in; resolves once signed in. `onPrompt` gets the page to open and any code. */
export function signInAssistant(kind: AssistantKind, onPrompt: (prompt: SignInPrompt) => void) {
  const channel = new Channel<SignInPrompt>();
  channel.onmessage = onPrompt;
  return invoke<AssistantInfo>("sign_in_assistant", { kind, onPrompt: channel });
}

export const submitSignInCode = (code: string) => invoke<void>("submit_sign_in_code", { code });

export const cancelSignIn = () => invoke<void>("cancel_sign_in");

/** Something a followed person did: a paper, a post, a change to their homepage. */
export interface Activity {
  id: string;
  person: string;
  source: "openalex" | "homepage" | "feed";
  kind: "paper" | "post" | "page_change";
  title: string;
  url: string;
  summary: string;
  /** "YYYY-MM-DD" when known. */
  published: string;
  found_at: number;
  /** Already there when following started. */
  baseline: boolean;
  /** "high", "medium", "low" or "none"; empty until Claude has judged it. */
  relevance: "" | "high" | "medium" | "low" | "none";
  reason: string;
  /** Names of the user's pages it relates to. */
  related: string[];
  seen: boolean;
  notified: boolean;
}

export interface Update {
  activity: Activity;
  person: Entity;
}

export interface WatchInfo {
  following: boolean;
  sources: { label: string; url: string; checked_at: number | null }[];
  /** Profile links that can't be read by apps: "X", "LinkedIn", "Google Scholar". */
  unsupported: string[];
  has_papers: boolean;
}

export interface AuthorCandidate {
  id: string;
  name: string;
  works_count: number;
  institutions: string[];
  orcid: string | null;
}

/** Profile link details, in the order they're shown. */
export const PROFILE_LINKS: { key: string; label: string; icon: string }[] = [
  { key: "homepage", label: "Homepage", icon: "globe" },
  { key: "scholar", label: "Google Scholar", icon: "student" },
  { key: "dblp", label: "DBLP", icon: "document" },
  { key: "openalex", label: "OpenAlex", icon: "document" },
  { key: "semantic_scholar", label: "Semantic Scholar", icon: "document" },
  { key: "orcid", label: "ORCID", icon: "person" },
  { key: "github", label: "GitHub", icon: "github" },
  { key: "twitter", label: "X", icon: "x" },
  { key: "linkedin", label: "LinkedIn", icon: "linkedin" },
  { key: "feed", label: "Feed", icon: "rss" },
];

export const followPerson = (id: string, follow: boolean) => invoke<WatchInfo>("follow_person", { id, follow });

export const addProfileLinks = (id: string, urls: string[]) => invoke<Entity>("add_profile_links", { id, urls });

export const removeProfileLink = (id: string, key: string) => invoke<Entity>("remove_profile_link", { id, key });

export const watchInfo = (id: string) => invoke<WatchInfo>("watch_info", { id });

export const openalexCandidates = (id: string) => invoke<AuthorCandidate[]>("openalex_candidates", { id });

export const checkPersonNow = (id: string) =>
  invoke<{ new: number; judged: number; errors: string[] }>("check_person_now", { id });

/** Relevant updates from followed people, or with `all` everything; for one person, also what was there before. */
export const listUpdates = (person: string | null, all: boolean) => invoke<Update[]>("list_updates", { person, all });

export const markUpdatesSeen = (ids: string[] | null) => invoke<void>("mark_updates_seen", { ids });

/** Sets a page's emoji icon; null removes it. */
export const setIcon = (id: string, icon: string | null) => invoke<Entity>("set_icon", { id, icon });

export const setSectionIcon = (tag: string, icon: string | null) => invoke<void>("set_section_icon", { tag, icon });

/** Stars a page, so it's listed under Favorites in the sidebar. */
export const setFavorite = (id: string, favorite: boolean) => invoke<Entity>("set_favorite", { id, favorite });

/** Edits a page's details: a string sets one, null removes it. */
export const updateDetails = (id: string, changes: Record<string, string | null>) =>
  invoke<Entity>("update_details", { id, changes });

/** A relationship or page type Suk started using, waiting to be kept, renamed or removed. */
export interface NewType {
  name: string;
  /** How it reads on a page: "reviews", "Grant". */
  label: string;
  count: number;
  /** A few examples: sentences for relationships, page names for types. */
  examples: string[];
}

/** Two pages that may be the same thing. */
export interface DuplicatePair {
  keep: Entity;
  remove: Entity;
  /** Why they look alike, e.g. "Satya is part of Satya Das". */
  reason: string;
}

export interface TidyItems {
  relations: NewType[];
  kinds: NewType[];
  duplicates: DuplicatePair[];
  /** Everything above, for the sidebar count. */
  count: number;
}

export const tidyItems = () => invoke<TidyItems>("tidy_items");

/** Keeps a new relationship or page type as it is, so it stops being offered for tidying. */
export const keepType = (what: "relation" | "kind", name: string) => invoke<void>("keep_type", { what, name });

/** Renames a type everywhere, or merges it into an existing one by giving that name. */
export const renameType = (what: "relation" | "kind", name: string, newName: string) =>
  invoke<void>("rename_type", { what, name, newName });

/** Removes a relationship type: the relationships of that kind are deleted (pages stay). */
export const removeRelationType = (name: string) => invoke<void>("remove_relation_type", { name });

/** Merges two pages into one, keeping everything both had. */
export const mergePages = (keep: string, remove: string) => invoke<Entity>("merge_pages", { keep, remove });

/** Marks two pages as different, so they're not offered as duplicates again. */
export const notDuplicates = (a: string, b: string) => invoke<void>("not_duplicates", { a, b });
