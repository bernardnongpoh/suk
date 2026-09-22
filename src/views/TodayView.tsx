import { useEffect, useMemo, useState } from "react";
import { listEntities, listTasks, listUpdates, openPage, submitDetails, type Entity, type TaskItem, type Update } from "../api";
import { timeOf, todayKey } from "../format";
import CalendarAsk from "../tasks/CalendarAsk";
import TaskList from "../tasks/TaskList";
import UpdateItem from "../updates/UpdateItem";
import Icon from "../ui/Icon";

interface Props {
  version: number;
  onPlan: () => void;
  onUpdates: () => void;
  onChanged: () => void;
  onOpen: (id: string) => void;
  /** Google Calendar isn't connected and the user wants to see to it. */
  onSettings: () => void;
}

/** "Good morning" and so on, by the hour. */
function greeting(hour = new Date().getHours()) {
  if (hour < 5) return "Working late";
  if (hour < 12) return "Good morning";
  if (hour < 17) return "Good afternoon";
  return "Good evening";
}

/** The four ways of looking at the day. Counts are worked out from the same lists the tabs show. */
const TABS = [
  { id: "today", label: "Today", hint: "Due today, and anything late" },
  { id: "important", label: "Important", hint: "High priority and overdue, whenever they're due" },
  { id: "dated", label: "Has a date", hint: "Everything with a day on it, soonest first" },
  { id: "others", label: "Follow up", hint: "With other people: assigned to them, or waiting on them" },
] as const;

type Tab = (typeof TABS)[number]["id"];

/** Overdue or due today. */
const dueBy = (item: TaskItem, day: string) => (item.task.info.due ?? "").slice(0, 10) <= day && !!item.task.info.due;

/** What each tab keeps from the list it is given. */
function keeps(tab: Tab, day: string): (item: TaskItem) => boolean {
  if (tab === "today") return (item) => dueBy(item, day);
  if (tab === "important") return (item) => item.task.info.priority === "high" || dueBy(item, day);
  if (tab === "dated") return (item) => !!item.task.info.due;
  return () => true;
}

const WHEN = [
  { label: "Today", days: 0 },
  { label: "Tomorrow", days: 1 },
  { label: "Next week", days: 7 },
  { label: "Someday", days: null },
] as const;

function TodayView({ version, onPlan, onUpdates, onChanged, onOpen, onSettings }: Props) {
  const [draft, setDraft] = useState("");
  const [when, setWhen] = useState<(typeof WHEN)[number]["label"]>("Today");
  const [adding, setAdding] = useState(false);
  const [events, setEvents] = useState<Entity[]>([]);
  const [updates, setUpdates] = useState<Update[]>([]);
  const [counts, setCounts] = useState<{ open: number; overdue: number } | null>(null);
  const [tab, setTab] = useState<Tab>("today");
  // Every open task, kept only to number the tabs.
  const [mine, setMine] = useState<TaskItem[]>([]);
  const [others, setOthers] = useState<TaskItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const today = todayKey();

  useEffect(() => {
    listEntities("Event").then(
      (events) =>
        setEvents(
          events
            .filter((e) => e.info.start?.startsWith(today))
            .sort((a, b) => a.info.start.localeCompare(b.info.start)),
        ),
      (err) => setError(String(err)),
    );
    listUpdates(null, false).then((all) => setUpdates(all.filter((u) => !u.activity.seen)), () => {});
    listTasks("open", undefined, "mine").then(setMine, () => {});
    listTasks("open", undefined, "others").then(setOthers, () => {});
  }, [today, version]);

  // A new function each render would send TaskList fetching again and again.
  const filter = useMemo(() => keeps(tab, today), [tab, today]);

  const count = (id: Tab) => (id === "others" ? others.length : mine.filter(keeps(id, today)).length);

  async function addTask() {
    const name = draft.trim();
    if (!name || adding) return;
    setAdding(true);
    try {
      const task = await openPage(name, "Task");
      const days = WHEN.find((w) => w.label === when)?.days;
      const due = new Date();
      if (days != null) due.setDate(due.getDate() + days);
      await submitDetails(task.id, { status: "open", ...(days != null && { due: todayKey(due) }) }, []);
      setDraft("");
      onChanged();
    } catch (err) {
      setError(String(err));
    } finally {
      setAdding(false);
    }
  }

  const date = new Date().toLocaleDateString(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
  });
  const summary = counts
    ? [`${counts.open} open ${counts.open === 1 ? "task" : "tasks"}`, counts.overdue > 0 && `${counts.overdue} overdue`]
        .filter(Boolean)
        .join(" · ")
    : "";

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <p className="eyebrow">{date}</p>
          <h1>{greeting()}</h1>
          {summary && <p className="muted">{summary}</p>}
        </div>
        <button className="button primary" onClick={onPlan}>
          <Icon name="sparkle" size={14} />
          Plan my day
        </button>
      </div>

      <form
        className={`quick-add${draft ? " typing" : ""}`}
        onSubmit={(e) => {
          e.preventDefault();
          addTask();
        }}
      >
        <span className="quick-add-check" aria-hidden="true">
          <Icon name="plus" size={13} />
        </span>
        <input
          value={draft}
          placeholder="Add a task…"
          aria-label="New task"
          disabled={adding}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => e.key === "Escape" && setDraft("")}
        />
        {draft && (
          <div className="quick-add-when" role="radiogroup" aria-label="When">
            {WHEN.map((w) => (
              <button
                key={w.label}
                type="button"
                role="radio"
                aria-checked={when === w.label}
                className={when === w.label ? "on" : ""}
                onClick={() => setWhen(w.label)}
              >
                {w.label}
              </button>
            ))}
            <kbd>↵</kbd>
          </div>
        )}
      </form>

      {error && <p className="form-error">{error}</p>}

      <div className="tabs" role="tablist" aria-label="What to look at">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            title={t.hint}
            className={`tab${tab === t.id ? " on" : ""}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
            {count(t.id) > 0 && <span className="tab-count">{count(t.id)}</span>}
          </button>
        ))}
      </div>

      {tab === "today" && events.length > 0 && (
        <>
          <h2 className="section-title">Scheduled</h2>
          <ul className="list">
            {events.map((e) => (
              <li key={e.id} className="row">
                <span className="row-time">
                  {timeOf(e.info.start)}–{e.info.end ? timeOf(e.info.end) : ""}
                </span>
                <span className="row-main">{e.info.title ?? e.name}</span>
                {e.info.calendar === "added" && <span className="row-meta">In calendar</span>}
              </li>
            ))}
          </ul>
        </>
      )}

      {tab === "today" && <CalendarAsk version={version} onChanged={onChanged} onSettings={onSettings} />}

      {tab === "others" ? (
        <TaskList
          assigned="others"
          version={version}
          onOpen={onOpen}
          empty={
            <div className="empty calm">
              <div className="empty-emoji">🤝</div>
              <p>Nothing with anyone else. Tasks you assign, or are waiting on someone for, wait here.</p>
            </div>
          }
        />
      ) : (
        <TaskList
          grouped={tab === "dated"}
          assigned="mine"
          version={version}
          filter={filter}
          onOpen={onOpen}
          onCount={(open, overdue) => setCounts({ open, overdue })}
          empty={
            <div className="empty calm">
              <div className="empty-emoji">{tab === "today" ? "🌤️" : tab === "important" ? "🎯" : "🗓️"}</div>
              <p>
                {tab === "today"
                  ? "Nothing due today. Add a task above, or tell the chat, like \"Review Satya's survey by Friday\"."
                  : tab === "important"
                    ? "Nothing urgent. Tasks marked high priority, and anything late, show up here."
                    : "Nothing has a date yet. Give a task a day and it appears here, soonest first."}
              </p>
            </div>
          }
        />
      )}

      {updates.length > 0 && (
        <section>
          <h2 className="section-title">
            From people you follow <span className="count">{updates.length}</span>
            <button className="text-button section-link" onClick={onUpdates}>
              See all
            </button>
          </h2>
          <ul className="updates compact">
            {updates.slice(0, 3).map((u) => (
              <UpdateItem key={u.activity.id} activity={u.activity} person={u.person} fresh onOpen={onOpen} onError={setError} />
            ))}
          </ul>
        </section>
      )}

    </div>
  );
}

export default TodayView;
