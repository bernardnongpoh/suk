import { useEffect, useMemo, useState } from "react";
import { listEntities, listTasks, type Entity, type TaskItem } from "../api";
import { timeOf, todayKey } from "../format";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";

interface Props {
  version: number;
  onOpen: (id: string) => void;
}

/** Monday of the week a date falls in. */
function weekStart(date: Date) {
  const start = new Date(date.getFullYear(), date.getMonth(), date.getDate());
  // getDay() is 0 on Sunday, which belongs to the week that began six days earlier.
  start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
  return start;
}

const addDays = (date: Date, days: number) => {
  const next = new Date(date.getFullYear(), date.getMonth(), date.getDate());
  next.setDate(next.getDate() + days);
  return next;
};

/** The days a month view shows: whole weeks, so the grid is square. */
function monthDays(month: Date) {
  const first = weekStart(new Date(month.getFullYear(), month.getMonth(), 1));
  const last = new Date(month.getFullYear(), month.getMonth() + 1, 0);
  const days: Date[] = [];
  for (let day = first; day <= last || days.length % 7 !== 0; day = addDays(day, 1)) {
    days.push(day);
  }
  return days;
}

/** One thing on a day: a task that's due, or a block of time that was confirmed. */
interface Item {
  id: string;
  name: string;
  day: string;
  /** "14:00" when it has one; sorted before the untimed.  */
  time: string | null;
  kind: "task" | "block";
  high: boolean;
  done: boolean;
  /** Whoever else is to do it. */
  people: Entity[];
}

function CalendarView({ version, onOpen }: Props) {
  const [span, setSpan] = useState<"week" | "month">("week");
  // The week or month being looked at, as any date inside it.
  const [anchor, setAnchor] = useState(() => new Date());
  const [tasks, setTasks] = useState<TaskItem[]>([]);
  const [blocks, setBlocks] = useState<Entity[]>([]);
  const [error, setError] = useState<string | null>(null);
  const today = todayKey();

  useEffect(() => {
    Promise.all([listTasks("open", undefined, "mine"), listTasks("open", undefined, "others")])
      .then(([mine, others]) => setTasks([...mine, ...others]), (err) => setError(String(err)));
    listEntities("Event").then(setBlocks, () => {});
  }, [version]);

  const days = span === "week" ? Array.from({ length: 7 }, (_, i) => addDays(weekStart(anchor), i)) : monthDays(anchor);

  const byDay = useMemo(() => {
    const items = new Map<string, Item[]>();
    const add = (item: Item) => items.set(item.day, [...(items.get(item.day) ?? []), item]);
    for (const { task, assigned_to } of tasks) {
      const due = task.info.due;
      if (!due) continue;
      add({
        id: task.id,
        name: task.name,
        day: due.slice(0, 10),
        time: due.length > 10 ? timeOf(due) : null,
        kind: "task",
        high: task.info.priority === "high",
        done: task.info.status === "done",
        people: assigned_to,
      });
    }
    for (const block of blocks) {
      const start = block.info.start;
      if (!start) continue;
      add({
        id: block.id,
        name: block.info.title ?? block.name,
        day: start.slice(0, 10),
        time: timeOf(start),
        kind: "block",
        high: false,
        done: false,
        people: [],
      });
    }
    // Timed things first, in order; then whatever has no time.
    for (const list of items.values()) {
      list.sort((a, b) => (a.time ?? "zz").localeCompare(b.time ?? "zz") || a.name.localeCompare(b.name));
    }
    return items;
  }, [tasks, blocks]);

  const title =
    span === "week"
      ? `${days[0].toLocaleDateString(undefined, { day: "numeric", month: days[0].getMonth() === days[6].getMonth() ? undefined : "short" })}–${days[6].toLocaleDateString(undefined, { day: "numeric", month: "long", year: days[6].getFullYear() === new Date().getFullYear() ? undefined : "numeric" })}`
      : anchor.toLocaleDateString(undefined, { month: "long", year: "numeric" });

  const move = (by: number) =>
    setAnchor((at) => (span === "week" ? addDays(at, by * 7) : new Date(at.getFullYear(), at.getMonth() + by, 1)));

  const entry = (item: Item) => (
    <button
      key={`${item.kind}:${item.id}`}
      className={`cal-entry ${item.kind}${item.high ? " high" : ""}${item.done ? " done" : ""}`}
      onClick={() => onOpen(item.id)}
      title={item.time ? `${item.time} · ${item.name}` : item.name}
    >
      {item.time && <span className="cal-time">{item.time}</span>}
      <span className="cal-name">{item.name}</span>
      {item.people.slice(0, 2).map((p) => (
        <Avatar key={p.id} entity={p} size={14} />
      ))}
    </button>
  );

  const dayCell = (day: Date) => {
    const key = todayKey(day);
    const items = byDay.get(key) ?? [];
    const shown = span === "month" ? items.slice(0, 3) : items;
    const outside = span === "month" && day.getMonth() !== anchor.getMonth();
    return (
      <div key={key} className={`cal-day${key === today ? " today" : ""}${outside ? " outside" : ""}`}>
        <div className="cal-date">
          {span === "week" && <span className="cal-weekday">{day.toLocaleDateString(undefined, { weekday: "short" })}</span>}
          <span className="cal-number">{day.getDate()}</span>
        </div>
        <div className="cal-entries">
          {shown.map(entry)}
          {items.length > shown.length && <span className="cal-more">+{items.length - shown.length} more</span>}
        </div>
      </div>
    );
  };

  return (
    <div className="page wide">
      <div className="page-header">
        <div>
          <h1>Calendar</h1>
          <p className="muted">{title}</p>
        </div>
        <div className="cal-controls">
          <button className="icon-button" aria-label={span === "week" ? "Week before" : "Month before"} onClick={() => move(-1)}>
            <Icon name="back" size={15} />
          </button>
          <button className="button small ghost" onClick={() => setAnchor(new Date())}>
            Today
          </button>
          <button className="icon-button" aria-label={span === "week" ? "Week after" : "Month after"} onClick={() => move(1)}>
            <Icon name="chevron" size={15} />
          </button>
          <div className="segmented" role="radiogroup" aria-label="How much to show">
            {(["week", "month"] as const).map((s) => (
              <button key={s} role="radio" aria-checked={span === s} className={span === s ? "on" : ""} onClick={() => setSpan(s)}>
                {s === "week" ? "Week" : "Month"}
              </button>
            ))}
          </div>
        </div>
      </div>

      {error && <p className="form-error">{error}</p>}

      {span === "month" && (
        <div className="cal-weekdays">
          {days.slice(0, 7).map((day) => (
            <span key={day.getDay()}>{day.toLocaleDateString(undefined, { weekday: "short" })}</span>
          ))}
        </div>
      )}

      <div className={`cal-grid ${span}`}>{days.map(dayCell)}</div>

      {byDay.size === 0 && (
        <div className="empty calm">
          <div className="empty-emoji">🗓️</div>
          <p>Nothing with a date yet. Give a task a day and it shows up here.</p>
        </div>
      )}
    </div>
  );
}

export default CalendarView;
