import { useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";

import { can } from "@/modules/organizations";
import { useSession } from "@/state/session";
import { useUi } from "@/state/ui";

import { ALL_NAV } from "./navigation";

function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

/**
 * App-wide keyboard shortcuts:
 *   ⌘K / Ctrl+K  command palette        c   create issue
 *   g then h/p/m/s  go to Your work / Projects / People / Workspace settings
 */
export function useGlobalShortcuts(): void {
  const navigate = useNavigate();
  const role = useSession((s) => s.organization?.role);
  const pendingGo = useRef<number | null>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const ui = useUi.getState();
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        ui.setCommandPaletteOpen(!ui.commandPaletteOpen);
        return;
      }
      if (event.metaKey || event.ctrlKey || event.altKey || isTyping(event.target) || ui.commandPaletteOpen || ui.createIssue) return;

      if (pendingGo.current !== null) {
        window.clearTimeout(pendingGo.current);
        pendingGo.current = null;
        const item = ALL_NAV.find((n) => n.shortcut === event.key.toLowerCase());
        if (item && (!item.requires || can(role, item.requires))) {
          event.preventDefault();
          navigate(item.to);
        }
        return;
      }
      if (event.key === "g") {
        pendingGo.current = window.setTimeout(() => (pendingGo.current = null), 1000);
      } else if (event.key === "c" && !ui.createIssue) {
        event.preventDefault();
        const projectKey = /^\/projects\/([^/]+)/.exec(window.location.pathname)?.[1]?.toUpperCase();
        ui.openCreateIssue({ projectKey });
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [navigate, role]);
}
