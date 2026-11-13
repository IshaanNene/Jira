import type { AppNotification } from "@/services/api";
import { titleCase } from "@/utils/format";

/** "Maya assigned you", "Ada changed the status to Done", … (the issue is shown separately). */
export function describe(n: Pick<AppNotification, "kind" | "actor_name" | "detail">): string {
  const who = n.actor_name || "Someone";
  switch (n.kind) {
    case "assigned":
      return `${who} assigned this to you`;
    case "mentioned":
      return `${who} mentioned you in a comment`;
    case "commented":
      return `${who} commented`;
    case "status_changed": {
      const to = (n.detail as { to_status?: unknown }).to_status;
      return typeof to === "string" ? `${who} changed the status to ${titleCase(to.replace(/_/g, " "))}` : `${who} changed the status`;
    }
    default:
      return `${who} updated this issue`;
  }
}

/** Where a notification leads: the issue page. */
export function issuePath(issueKey: string): string {
  const project = issueKey.slice(0, issueKey.lastIndexOf("-"));
  return `/projects/${project}/issues/${issueKey}`;
}

export function badgeLabel(count: number): string | null {
  if (count <= 0) return null;
  return count > 9 ? "9+" : String(count);
}
