import { useEffect, useState } from "react";
import { listTasks, setTaskDone, type Entity, type TaskItem } from "../api";
import { dueLabel, todayKey } from "../format";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";

interface Props {
  /** Only tasks for this person, or of this project or course. */
  about?: string;
  /** Group by when they're due, as on Today. */
  grouped?: boolean;
  /** Only the professor's own tasks, or only those assigned to other people. */
  assigned?: "mine" | "others";
  version: number;
  onOpen: (id: string) => void;
  /** Shown when there are no open tasks; nothing is shown if absent. */
  empty?: React.ReactNode;
  onCount?: (open: number, overdue: number) => void;
  /** Shown above the list when it isn't empty. */
  header?: React.ReactNode;
}

const addDays = (days: number) => {
  const d = new Date();
  d.setDate(d.getDate() + days);
  return todayKey(d);
};

/** "Overdue", "Today", "Tomorrow", "This week", "Later", "No date". */
function group(item: TaskItem) {
  const due = item.task.info.due?.slice(0, 10);
  if (!due) return "No date";
  if (due < todayKey()) return "Overdue";
  if (due === todayKey()) return "Today";
  if (due === addDays(1)) return "Tomorrow";
  if (due <= addDays(7)) return "This week";
  return "Later";
}

/**
 * Open tasks, in the order the database returns them (soonest first). Ticking one crosses it
 * out; it leaves the list the next time the list loads.
 */
function TaskList({ about, grouped, assigned, version, onOpen, empty, onCount, header }: Props) {
  const [items, setItems] = useState<TaskItem[] | null>(null);
  const [done, setDone] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    listTasks("open", about, assigned).then(
      (result) => {
        if (cancelled) return;
        setItems(result);
        setDone({});
        const today = todayKey();
        onCount?.(result.length, result.filter((i) => (i.task.info.due ?? "9999").slice(0, 10) < today).length);
      },
      (err) => !cancelled && setError(String(err)),
    );
    return () => {
      cancelled = true;
    };
  }, [about, assigned, version]);

  async function toggle(task: Entity) {
    const next = !done[task.id];
    setDone((d) => ({ ...d, [task.id]: next }));
    try {
      await setTaskDone(task.id, next);
    } catch (err) {
      setDone((d) => ({ ...d, [task.id]: !next }));
      setError(String(err));
    }
  }

  if (error) return <p className="form-error">{error}</p>;
  if (!items) return null;
  if (items.length === 0) return empty ? <>{empty}</> : null;

  const person = (e: Entity, label?: string) =>
    e.id === about ? null : (
      <button key={e.id} className="task-chip" onClick={() => onOpen(e.id)}>
        {label && <span className="task-chip-label">{label}</span>}
        <Avatar entity={e} size={16} />
        {e.name}
      </button>
    );

  const row = (item: TaskItem) => {
    const { task } = item;
    const checked = !!done[task.id];
    const due = task.info.due;
    const overdue = !!due && due.slice(0, 10) < todayKey();
    return (
      <li key={task.id} className={`task${checked ? " done" : ""}`}>
        <button
          className="task-check"
          role="checkbox"
          aria-checked={checked}
          aria-label={checked ? `Reopen ${task.name}` : `Mark ${task.name} done`}
          onClick={() => toggle(task)}
        >
          <Icon name="check" size={12} />
        </button>
        <div className="task-main">
          <button className="task-title" onClick={() => onOpen(task.id)}>
            {task.name}
          </button>
          <div className="task-meta">
            {task.info.priority === "high" && (
              <span className="task-flag high">
                <Icon name="flag" size={11} /> High
              </span>
            )}
            {task.info.status === "waiting" && item.waiting_on.length === 0 && <span className="task-flag">Waiting</span>}
            {item.assigned_to.some((p) => p.id === about) && <span className="task-flag assigned">Assigned</span>}
            {item.assigned_to.some((p) => p.id !== about) && (
              <span className="task-assignees">
                <span className="task-chip-label">{item.assigned_to.some((p) => p.id === about) ? "with" : "Assigned to"}</span>
                {item.assigned_to.filter((p) => p.id !== about).map((p) => (
                  <button key={p.id} className="task-chip" onClick={() => onOpen(p.id)}>
                    <Avatar entity={p} size={16} />
                    {p.name}
                  </button>
                ))}
              </span>
            )}
            {item.for.map((p) => person(p, "For"))}
            {item.waiting_on.map((p) => person(p, "Waiting on"))}
            {item.part_of.map((p) => person(p))}
            {task.info.area && <span className="task-area">{task.info.area}</span>}
          </div>
        </div>
        {due && <span className={`task-due${overdue ? " overdue" : ""}`}>{dueLabel(due)}</span>}
      </li>
    );
  };

  if (!grouped) {
    return (
      <>
        {header}
        <ul className="task-list">{items.map(row)}</ul>
      </>
    );
  }

  const order = ["Overdue", "Today", "Tomorrow", "This week", "Later", "No date"];
  return (
    <div className="task-groups">
      {order.map((name) => {
        const inGroup = items.filter((i) => group(i) === name);
        if (!inGroup.length) return null;
        return (
          <section key={name}>
            <h2 className={`section-title${name === "Overdue" ? " overdue" : ""}`}>
              {name} <span className="count">{inGroup.length}</span>
            </h2>
            <ul className="task-list">{inGroup.map(row)}</ul>
          </section>
        );
      })}
    </div>
  );
}

export default TaskList;
