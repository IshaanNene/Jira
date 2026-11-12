import { create } from "zustand";

export type Theme = "light" | "dark" | "system";

const THEME_KEY = "platform.theme";
const SIDEBAR_KEY = "platform.sidebarCollapsed";

function stored(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function persist(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Storage can be unavailable (private mode); the preference then lasts for this tab only.
  }
}

function applyTheme(theme: Theme): void {
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

interface UiState {
  theme: Theme;
  sidebarCollapsed: boolean;
  commandPaletteOpen: boolean;
  /** Create-issue dialog: closed, or open with an optional preselected project / parent. */
  createIssue: { projectKey?: string; parentKey?: string } | null;
  setTheme: (theme: Theme) => void;
  toggleSidebar: () => void;
  setCommandPaletteOpen: (open: boolean) => void;
  openCreateIssue: (defaults?: { projectKey?: string; parentKey?: string }) => void;
  closeCreateIssue: () => void;
}

const initialTheme = ((): Theme => {
  const value = stored(THEME_KEY);
  return value === "light" || value === "dark" ? value : "system";
})();

/** Interface chrome preferences. Purely client-side; nothing here comes from the API. */
export const useUi = create<UiState>((set) => ({
  theme: initialTheme,
  sidebarCollapsed: stored(SIDEBAR_KEY) === "1",
  commandPaletteOpen: false,
  createIssue: null,
  setTheme: (theme) => {
    persist(THEME_KEY, theme);
    applyTheme(theme);
    set({ theme });
  },
  toggleSidebar: () =>
    set((s) => {
      persist(SIDEBAR_KEY, s.sidebarCollapsed ? "0" : "1");
      return { sidebarCollapsed: !s.sidebarCollapsed };
    }),
  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
  openCreateIssue: (defaults = {}) => set({ createIssue: defaults }),
  closeCreateIssue: () => set({ createIssue: null }),
}));

export function initTheme(): void {
  applyTheme(useUi.getState().theme);
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => applyTheme(useUi.getState().theme));
}
