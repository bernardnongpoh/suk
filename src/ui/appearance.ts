// Light or dark, and the accent color: per-device preferences kept in local storage.

export type Theme = "system" | "light" | "dark";

export const ACCENTS = [
  { id: "indigo", label: "Indigo", color: "#5b5bd6" },
  { id: "blue", label: "Blue", color: "#2f7de1" },
  { id: "teal", label: "Teal", color: "#0f9488" },
  { id: "green", label: "Green", color: "#2f9e5b" },
  { id: "amber", label: "Amber", color: "#d97706" },
  { id: "rose", label: "Rose", color: "#e0457b" },
  { id: "graphite", label: "Graphite", color: "#52525b" },
] as const;

export type Accent = (typeof ACCENTS)[number]["id"];

export interface Appearance {
  theme: Theme;
  accent: Accent;
}

const KEY = "professor-os.appearance";

export function loadAppearance(): Appearance {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return {
      theme: ["system", "light", "dark"].includes(saved.theme) ? saved.theme : "system",
      accent: ACCENTS.some((a) => a.id === saved.accent) ? saved.accent : "indigo",
    };
  } catch {
    return { theme: "system", accent: "indigo" };
  }
}

/** Applies the appearance to the page, and remembers it when possible. */
export function applyAppearance(appearance: Appearance, save = true) {
  const root = document.documentElement;
  if (appearance.theme === "system") delete root.dataset.theme;
  else root.dataset.theme = appearance.theme;
  root.dataset.accent = appearance.accent;
  if (!save) return;
  try {
    localStorage.setItem(KEY, JSON.stringify(appearance));
  } catch {
    // Without storage the choice lasts until the app closes.
  }
}

/** Whether the app is showing dark colors right now. */
export const isDark = () =>
  document.documentElement.dataset.theme === "dark" ||
  (!document.documentElement.dataset.theme && window.matchMedia("(prefers-color-scheme: dark)").matches);
