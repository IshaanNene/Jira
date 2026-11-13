import { Bell, FolderKanban, GitBranch, Home, MonitorSmartphone, ScrollText, Settings, SlidersHorizontal, UserRound, Users, type LucideIcon } from "lucide-react";

import type { OrgPermission } from "@/modules/organizations";

export interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Hidden unless the current role has this permission. */
  requires?: OrgPermission;
  /** Two-key "go to" shortcut: press g, then this key. */
  shortcut?: string;
  end?: boolean;
}

/**
 * The single list of destinations. The top bar, command palette and keyboard shortcuts
 * are derived from it, so they cannot drift apart. Entries are added as each phase
 * delivers a working screen — never before.
 */
export const MAIN_NAV: NavItem[] = [
  { to: "/", label: "Your work", icon: Home, shortcut: "h", end: true },
  { to: "/projects", label: "Projects", icon: FolderKanban, shortcut: "p" },
  { to: "/people", label: "People", icon: Users, requires: "viewMembers", shortcut: "m" },
];

export const SETTINGS_NAV: NavItem[] = [
  { to: "/settings/workspace", label: "Workspace settings", icon: Settings, shortcut: "s" },
  { to: "/settings/workflows", label: "Workflows", icon: GitBranch },
  { to: "/settings/custom-fields", label: "Custom fields", icon: SlidersHorizontal, requires: "manageOrganization" },
  { to: "/settings/audit-log", label: "Audit log", icon: ScrollText, requires: "viewAuditLog" },
  { to: "/settings/profile", label: "Profile", icon: UserRound },
  { to: "/settings/notifications", label: "Notifications", icon: Bell },
  { to: "/settings/sessions", label: "Sessions", icon: MonitorSmartphone },
];

export const ALL_NAV: NavItem[] = [...MAIN_NAV, ...SETTINGS_NAV];
