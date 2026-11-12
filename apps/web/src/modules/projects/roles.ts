import type { ProjectRole } from "@/services/api";

// Display-only mirror of the server's project role table; the API enforces it.
const RANK: Record<ProjectRole, number> = { viewer: 0, member: 1, administrator: 2 };

export const PROJECT_ROLE_LABEL: Record<ProjectRole, string> = { administrator: "Administrator", member: "Member", viewer: "Viewer" };
export const PROJECT_ROLE_DESCRIPTION: Record<ProjectRole, string> = {
  administrator: "Manage the project, its people and all issues",
  member: "Create, edit, move and comment on issues",
  viewer: "Read-only access",
};
export const PROJECT_TYPE_LABEL: Record<string, string> = { scrum: "Scrum", kanban: "Kanban", general: "General" };

export function roleAtLeast(role: string | undefined, minimum: ProjectRole): boolean {
  return role !== undefined && role in RANK && RANK[role as ProjectRole] >= RANK[minimum];
}

/** Suggest a project key from its name: initials of the words, or the first letters. */
export function suggestKey(name: string): string {
  const words = name.toUpperCase().replace(/[^A-Z0-9 ]/g, " ").split(/\s+/).filter(Boolean);
  const raw = words.length > 1 ? words.map((w) => w[0]).join("") : (words[0] ?? "").slice(0, 4);
  const key = raw.replace(/^[0-9]+/, "").slice(0, 10);
  return key.length >= 2 ? key : "";
}
