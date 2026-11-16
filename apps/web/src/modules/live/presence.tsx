import { useQuery, type QueryKey } from "@tanstack/react-query";
import { useEffect } from "react";

import { Avatar } from "@/components/ui";
import { useUserDirectory } from "@/modules/organizations";
import { api } from "@/services/api";
import { useSession } from "@/state/session";

const PATH = "/api/v1/rt/presence";
/** The server forgets someone 60 s after their last "I am here". */
const REPEAT_MS = 30_000;

export const presenceKey = (orgId: string | undefined, issueId: string): QueryKey => ["org", orgId, "presence", issueId];

/**
 * Says "I am looking at this issue" while mounted and visible (every 30 s, and again when a
 * `presence.changed` hint marks the answer stale), says "I left" on unmount, and returns
 * the user ids of the others looking at it.
 */
export function usePresence(projectId: string | undefined, issueId: string): string[] {
  const orgId = useSession((s) => s.organization?.id);
  const enabled = Boolean(orgId && projectId);
  const query = useQuery({
    queryKey: presenceKey(orgId, issueId),
    queryFn: () => api.put<{ viewers: string[] }>(PATH, { project_id: projectId, issue_id: issueId }),
    enabled,
    refetchInterval: REPEAT_MS,
    // A hidden tab stops repeating, so it drops out after a minute.
    refetchIntervalInBackground: false,
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
  useEffect(() => {
    if (!enabled) return undefined;
    return () => {
      // Best effort: if it fails, presence runs out on its own.
      void api.delete(PATH, { body: { project_id: projectId, issue_id: issueId } }).catch(() => undefined);
    };
  }, [enabled, projectId, issueId]);
  return query.data?.viewers ?? [];
}

/** "Also here" avatars for an issue; nothing when no one else is looking. */
export function IssueViewers({ projectId, issueId }: { projectId: string | undefined; issueId: string }) {
  const viewers = usePresence(projectId, issueId);
  const { byId } = useUserDirectory();
  const people = viewers.map((id) => ({ id, name: byId.get(id)?.display_name ?? "Someone" }));
  if (people.length === 0) return null;
  const shown = people.slice(0, 5);
  const names = people.map((p) => p.name).join(", ");
  return (
    <div className="flex items-center gap-1.5" aria-label={`Also viewing: ${names}`} title={`Also viewing: ${names}`}>
      <span className="text-xs text-ink-muted">Also here</span>
      <span className="flex -space-x-1.5">
        {shown.map((p) => <Avatar key={p.id} name={p.name} seed={p.id} size="sm" className="ring-2 ring-surface" />)}
      </span>
      {people.length > shown.length && <span className="text-xs text-ink-muted">+{people.length - shown.length}</span>}
    </div>
  );
}
