import { Bell, Gauge, GitPullRequest, KeySquare, UserPlus, FolderKanban, LayoutDashboard, GitBranch, Home, ScrollText, Settings, ShieldCheck, SlidersHorizontal, UserRound, Users, Webhook, type LucideIcon } from "lucide-react";

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
  { to: "/dashboards", label: "Dashboards", icon: LayoutDashboard, shortcut: "d" },
  { to: "/people", label: "People", icon: Users, requires: "viewMembers", shortcut: "m" },
];

export const SETTINGS_NAV: NavItem[] = [
  { to: "/settings/workspace", label: "Workspace settings", icon: Settings, shortcut: "s" },
  { to: "/settings/admin", label: "Admin console", icon: Gauge, requires: "manageOrganization" },
  { to: "/settings/workflows", label: "Workflows", icon: GitBranch },
  { to: "/settings/custom-fields", label: "Custom fields", icon: SlidersHorizontal, requires: "manageOrganization" },
  { to: "/settings/sso", label: "Single sign-on", icon: KeySquare, requires: "manageOrganization" },
  { to: "/settings/provisioning", label: "Provisioning", icon: UserPlus, requires: "manageOrganization" },
  { to: "/settings/webhooks", label: "Webhooks", icon: Webhook, requires: "manageOrganization" },
  { to: "/settings/source-control", label: "Source control", icon: GitPullRequest, requires: "manageOrganization" },
  { to: "/settings/audit-log", label: "Audit log", icon: ScrollText, requires: "viewAuditLog" },
  { to: "/settings/profile", label: "Profile", icon: UserRound },
  { to: "/settings/notifications", label: "Notifications", icon: Bell },
  { to: "/settings/security", label: "Security", icon: ShieldCheck },
];

export const ALL_NAV: NavItem[] = [...MAIN_NAV, ...SETTINGS_NAV];
