// Display helpers for the "YYYY-MM-DD" / "YYYY-MM-DDTHH:MM" local times stored in the graph.

const pad = (n: number) => String(n).padStart(2, "0");

/** Today's date as "YYYY-MM-DD" in local time. */
export function todayKey(date = new Date()) {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** "10:00" from "2026-09-17T10:00". */
export const timeOf = (stamp: string) => stamp.slice(11, 16);

/** "Today", "Tomorrow", "Fri 18 Sep" or "Overdue · Tue 15 Sep" for a due date. */
export function dueLabel(due: string, now = new Date()) {
  const day = due.slice(0, 10);
  const today = todayKey(now);
  const tomorrow = todayKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1));
  const time = due.length > 10 ? ` ${timeOf(due)}` : "";
  if (day === today) return `Today${time}`;
  if (day === tomorrow) return `Tomorrow${time}`;
  const [y, m, d] = day.split("-").map(Number);
  const date = new Date(y, m - 1, d);
  if (Number.isNaN(date.getTime())) return due;
  const label = date.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
  return day < today ? `Overdue · ${label}` : `${label}${time}`;
}

/** "Thu 17 Sep, 10:00–11:30" for a schedule item. */
export function spanLabel(start: string, end: string) {
  const day = start.slice(0, 10) === todayKey() ? "Today" : dueLabel(start.slice(0, 10));
  return `${day}, ${timeOf(start)}–${timeOf(end)}`;
}

/** "program" -> "Program", "start_year" -> "Start year". */
export const keyLabel = (key: string) =>
  key.charAt(0).toUpperCase() + key.slice(1).replace(/_/g, " ");

/** "just now", "5 min ago", "3 h ago", "2 days ago" for a millisecond timestamp. */
export function agoLabel(at: number, now = Date.now()) {
  const minutes = Math.round((now - at) / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  return `${days} ${days === 1 ? "day" : "days"} ago`;
}

/** "16 Sep" this year, "16 Sep 2025" otherwise, for "YYYY-MM-DD". */
export function dateLabel(day: string, now = new Date()) {
  const [y, m, d] = day.slice(0, 10).split("-").map(Number);
  const date = new Date(y, (m || 1) - 1, d || 1);
  if (!y || Number.isNaN(date.getTime())) return day;
  if (day.slice(0, 10) === todayKey(now)) return "Today";
  return date.toLocaleDateString(undefined, { day: "numeric", month: "short", ...(y !== now.getFullYear() && { year: "numeric" }) });
}
