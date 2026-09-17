// Emoji for page icons: the full set (loaded on first use), search, and icons suggested from what
// a page is about.

export interface Emoji {
  emoji: string;
  label: string;
  tags: string[];
  group: number;
}

/** Emoji groups shown as tabs, in Unicode order (skin tones and flags left out). */
export const GROUPS: { id: number; label: string; sample: string }[] = [
  { id: 0, label: "Smileys", sample: "😀" },
  { id: 1, label: "People", sample: "🧑‍🎓" },
  { id: 3, label: "Nature", sample: "🌿" },
  { id: 4, label: "Food", sample: "☕" },
  { id: 5, label: "Places", sample: "🏛️" },
  { id: 6, label: "Activities", sample: "🎯" },
  { id: 7, label: "Objects", sample: "💡" },
  { id: 8, label: "Symbols", sample: "✳️" },
];

let loading: Promise<Emoji[]> | null = null;

export function loadEmoji(): Promise<Emoji[]> {
  loading ??= import("emojibase-data/en/compact.json").then((module) =>
    (module.default as { unicode: string; label: string; tags?: string[]; group?: number }[])
      .filter((e) => e.group !== undefined && e.group !== 2 && e.group !== 9)
      .map((e) => ({ emoji: e.unicode, label: e.label, tags: e.tags ?? [], group: e.group! })),
  );
  return loading;
}

/** Emoji whose name or keywords start with every word typed. */
export function searchEmoji(all: Emoji[], query: string) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const score = (e: Emoji) => {
    const names = [...e.label.toLowerCase().split(/[\s:-]+/), ...e.tags];
    let total = 0;
    for (const word of words) {
      if (e.label.toLowerCase() === word) total += 3;
      else if (names.some((n) => n === word)) total += 2;
      else if (names.some((n) => n.startsWith(word))) total += 1;
      else return 0;
    }
    return total;
  };
  return all
    .map((e) => [e, score(e)] as const)
    .filter(([, s]) => s > 0)
    .sort((a, b) => b[1] - a[1])
    .map(([e]) => e);
}

/** Icons for subjects a professor's pages are often about, matched on word starts. */
const SUBJECTS: [string[], string[]][] = [
  [["fuzz"], ["🐛", "🎲", "🧪"]],
  [["bug", "debug"], ["🐞", "🔍"]],
  [["compiler", "llvm", "gcc"], ["⚙️", "🛠️"]],
  [["solidity", "blockchain", "ethereum", "smart contract", "web3", "crypto"], ["⛓️", "📜", "🪙"]],
  [["secur", "privacy", "attack", "vulnerab", "malware"], ["🔒", "🛡️"]],
  [["analysis", "analyz", "static", "audit"], ["🔍", "🔎"]],
  [["llm", "gpt", "language model", "agent", " ai ", "artificial"], ["🤖", "✨"]],
  [["learning", "neural", "deep", "machine"], ["🧠", "🤖"]],
  [["repair", "fix", "patch", "maintenance"], ["🔧", "🩹"]],
  [["grammar", "parser", "parsing", "syntax"], ["🔤", "🧩"]],
  [["test", "verification", "verify", "formal", "proof"], ["✅", "🧪", "📐"]],
  [["program", "software", "code", "coding"], ["💻", "⌨️"]],
  [["data", "database", "statistic"], ["📊", "🗄️"]],
  [["network", "internet", "web", "distributed"], ["🌐", "🕸️"]],
  [["cloud", "server", "system"], ["☁️", "🖥️"]],
  [["quantum"], ["⚛️"]],
  [["bio", "protein", "gene", "health", "medical"], ["🧬", "🩺"]],
  [["robot", "drone", "embedded", "iot", "hardware"], ["🤖", "🔌"]],
  [["vision", "image", "graphics"], ["👁️", "🖼️"]],
  [["math", "theory", "algorithm"], ["📐", "➗"]],
  [["energy", "power", "climate"], ["⚡", "🌱"]],
  [["grant", "fund", "budget", "finance", "money", "serb", "dst"], ["💰", "🏦"]],
  [["paper", "survey", "literature", "article", "writing", "draft"], ["📄", "✍️"]],
  [["thesis", "phd", "dissertation", "viva"], ["🎓", "📘"]],
  [["course", "lecture", "teaching", "class", "lab session", "tutorial"], ["📚", "🧑‍🏫"]],
  [["exam", "quiz", "grading", "assignment"], ["📝", "🗒️"]],
  [["reading", "book", "seminar"], ["📖", "☕"]],
  [["meeting", "sync", "review"], ["🗓️", "💬"]],
  [["conference", "talk", "workshop", "presentation", "slides", "keynote"], ["🎤", "🎟️"]],
  [["travel", "trip", "visit"], ["✈️", "🧳"]],
  [["committee", "admin", "accreditation", "nba", "naac", "policy", "senate"], ["🏛️", "📋"]],
  [["hiring", "recruit", "admission", "interview"], ["🤝", "📨"]],
  [["startup", "product", "launch"], ["🚀", "🌟"]],
  [["university", "institute", "iit", "iisc", "college", "school"], ["🏛️", "🎓"]],
  [["department", "cse", "faculty of"], ["🏫"]],
  [["lab", "laboratory", "group"], ["🔬", "🧪"]],
  [["company", "qualcomm", "google", "microsoft", "intel", "amazon", "ibm", "tcs", "infosys"], ["🏢", "💼"]],
  [["collaborat", "partner"], ["🤝"]],
  [["idea", "brainstorm"], ["💡", "✨"]],
];

/** Icons that suit a kind of page, or a person's role. */
const BY_KIND: Record<string, string[]> = {
  Project: ["🚀", "📁", "🧪"],
  Course: ["📚", "🧑‍🏫", "🎒"],
  Idea: ["💡", "✨", "🌱"],
  Task: ["✅", "📌", "⏰"],
  Event: ["📅", "🎤", "☕"],
  Note: ["📝", "🗒️", "📎"],
  Document: ["📄", "📎", "🗂️"],
  Organization: ["🏛️", "🏢", "🏫"],
  ResearchArea: ["🔭", "🧭", "🧠"],
  student: ["🧑‍🎓", "🎓", "📘"],
  collaborator: ["🤝", "🧑‍🔬", "🌍"],
  colleague: ["🧑‍🏫", "☕", "🤝"],
  faculty: ["🧑‍🏫", "🏛️", "🧑‍🔬"],
  staff: ["🗂️", "🧑‍💼", "📋"],
  alumni: ["🎓", "🌟", "🎉"],
  Person: ["🙂", "🧑‍💻", "🧑‍🔬"],
};

/** Up to `max` icons proposed for a page from its name, tags and kind. */
export function suggestIcons(
  page: { kind: string; name: string; tags?: string[]; info?: Record<string, string>; notes?: string },
  all: Emoji[] = [],
  max = 10,
) {
  const text = ` ${[page.name, ...(page.tags ?? []), page.info?.type ?? "", page.info?.program ?? "", (page.notes ?? "").slice(0, 200)]
    .join(" ")
    .toLowerCase()} `;
  const found: string[] = [];
  const add = (...icons: string[]) => {
    for (const icon of icons) if (!found.includes(icon)) found.push(icon);
  };
  for (const [words, icons] of SUBJECTS) {
    if (words.some((w) => text.includes(w.startsWith(" ") || w.endsWith(" ") ? w : ` ${w}`) || text.includes(`-${w}`))) add(...icons);
  }
  // Emoji named after a word of the name ("rocket", "coffee"), for names outside the list.
  const words = page.name.toLowerCase().split(/[^a-z]+/).filter((w) => w.length >= 4);
  for (const e of all) {
    if (found.length >= max) break;
    if (words.some((w) => e.label === w || e.tags.includes(w))) add(e.emoji);
  }
  // Tags are lowercase ("project", "research-area"); kinds capitalized.
  const kindOf = (key: string) => key.split("-").map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join("");
  for (const key of [...(page.tags ?? []), page.kind]) add(...(BY_KIND[key] ?? BY_KIND[kindOf(key)] ?? []));
  if (found.length < 4) add("⭐", "📌", "🗂️");
  return found.slice(0, max);
}

const RECENT_KEY = "professor-os.recent-icons";

export function recentIcons(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
  } catch {
    return [];
  }
}

export function rememberIcon(icon: string) {
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify([icon, ...recentIcons().filter((i) => i !== icon)].slice(0, 16)));
  } catch {
    // Recent icons are a convenience; without storage the picker still works.
  }
}
