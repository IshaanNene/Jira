import type { AuditEntry } from "@/services/api";

/** Filterable groups of actions, as the API names them. */
export const AUDIT_CATEGORIES: { value: string; label: string }[] = [
  { value: "organization", label: "Workspace" },
  { value: "member", label: "People" },
  { value: "project", label: "Projects and access" },
  { value: "version", label: "Releases" },
  { value: "workflow", label: "Workflows" },
  { value: "board", label: "Boards" },
  { value: "sprint", label: "Sprints" },
  { value: "issue", label: "Deleted issues" },
  { value: "comment", label: "Deleted comments" },
];

const ROLE: Record<string, string> = {
  owner: "Owner",
  admin: "Admin",
  member: "Member",
  guest: "Guest",
  administrator: "Administrator",
  viewer: "Viewer",
  none: "no one",
};

const role = (value: string | undefined) => (value ? (ROLE[value] ?? value) : "");

/** Who did it, in words. */
export function actorName(entry: AuditEntry, people: Record<string, string>): string {
  if (entry.actor.type === "user") return people[entry.actor.id] ?? "Someone who has since left";
  if (entry.actor.type === "automation") return "An automation rule";
  return "The system";
}

/** What the entry is about, in words ("PAY", "Maya Chen", "board"). */
export function resourceName(entry: AuditEntry, people: Record<string, string>): string {
  if (entry.resource_type === "user") return people[entry.resource_id] ?? "a former member";
  return entry.resource_label || `a ${entry.resource_type}`;
}

/**
 * The entry as a sentence that follows the actor's name: "changed Maya Chen's role to
 * Admin". Unknown actions fall back to their name, so new ones still read sensibly.
 */
export function describe(entry: AuditEntry, people: Record<string, string>): string {
  const what = resourceName(entry, people);
  const d = entry.details;
  const grantee = d.user_id ? (people[d.user_id] ?? "a former member") : "someone";
  switch (entry.action) {
    case "organization.created":
      return `created the workspace ${what}`;
    case "organization.renamed":
      return `renamed the workspace to ${what}`;
    case "organization.two_step_required":
      return "required two-step verification of every member";
    case "organization.two_step_optional":
      return "stopped requiring two-step verification";
    case "member.invited":
      return `invited ${what} as ${role(d.role)}`;
    case "member.joined": {
      const who = entry.actor.id === entry.resource_id ? "joined" : `added ${what}`;
      return d.role ? `${who} as ${role(d.role)}` : who;
    }
    case "member.role_changed":
      return `changed ${what}'s role to ${role(d.role)}`;
    case "member.suspended":
      return `suspended ${what}`;
    case "member.reactivated":
      return `reactivated ${what}`;
    case "member.removed":
    case "member.left":
      return entry.actor.id === entry.resource_id ? "left the workspace" : `removed ${what} from the workspace`;
    case "project.created":
      return `created project ${what}${d.name ? ` (${d.name})` : ""}`;
    case "project.updated":
      return d.status === "archived" ? `updated project ${what} (archived)` : `updated project ${what}`;
    case "project.deleted":
      return `deleted project ${what}`;
    case "project.access_granted":
      return `gave ${grantee} ${role(d.role)} access to ${what}`;
    case "project.access_removed":
      return `removed ${grantee}'s access to ${what}`;
    case "project.default_access_changed":
      return d.default_role === "none" ? `made ${what} private` : `let every member into ${what} as ${role(d.default_role)}`;
    case "version.released":
      return `released version ${what}`;
    case "workflow.published":
      return `published workflow ${what}${d.version ? ` (version ${d.version})` : ""}`;
    case "workflow.created":
    case "workflow.renamed":
    case "workflow.assigned":
    case "workflow.unassigned":
    case "workflow.deleted":
      return `${entry.action.split(".")[1]} a workflow`;
    case "board.created":
    case "board.renamed":
    case "board.deleted":
      return `${entry.action.split(".")[1]} a board`;
    case "board.columns_changed":
      return "changed a board's columns";
    case "board.quick_filters_changed":
      return "changed a board's quick filters";
    case "sprint.started":
      return `started sprint ${what}${d.issues ? ` with ${d.issues} issues` : ""}`;
    case "sprint.completed":
      return `completed sprint ${what}`;
    case "issue.deleted":
      return `deleted issue ${what}`;
    case "comment.deleted":
      return `deleted a comment on ${what}`;
    default:
      return `${entry.action.replace(/[._]/g, " ")} ${what}`;
  }
}
