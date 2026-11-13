import { useQueryClient } from "@tanstack/react-query";
import { WifiOff } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useParams } from "react-router-dom";

import { issueKeys } from "@/modules/issues";
import { projectKeys, useProject } from "@/modules/projects";
import { useSession } from "@/state/session";

import { RefreshBatch, staleFor, type ProjectKeys } from "./refresh";
import { useLiveChannel } from "./useLiveChannel";

/**
 * Keeps the open project's screens current: other people's changes refetch the affected
 * queries. Renders only a small notice while live updates are interrupted.
 */
export function ProjectLive() {
  const { projectKey = "" } = useParams();
  const key = projectKey.toUpperCase();
  const orgId = useSession((s) => s.organization?.id);
  const project = useProject(key);
  const qc = useQueryClient();
  const batch = useMemo(() => new RefreshBatch(qc), [qc]);
  useEffect(() => () => batch.dispose(), [batch]);

  const keys: ProjectKeys = {
    lists: issueKeys.lists(orgId, key),
    issue: (issueKey) => issueKeys.one(orgId, issueKey),
    project: projectKeys.one(orgId, key),
  };
  const status = useLiveChannel(
    project.data ? `project:${project.data.id}` : undefined,
    (hint) => batch.add(...staleFor(hint, keys)),
    () => batch.add(keys.lists, keys.project, issueKeys.all(orgId)),
  );

  // Only after a few seconds: a quick reconnect should not flash a warning.
  const [showOffline, setShowOffline] = useState(false);
  useEffect(() => {
    if (status !== "offline") {
      setShowOffline(false);
      return undefined;
    }
    const timer = setTimeout(() => setShowOffline(true), 5000);
    return () => clearTimeout(timer);
  }, [status]);

  if (!showOffline) return null;
  return (
    <div role="status" className="fixed bottom-4 left-4 z-50 flex items-center gap-2 rounded border border-line bg-surface px-3 py-2 text-xs text-ink-muted shadow-md">
      <WifiOff className="h-3.5 w-3.5" aria-hidden />
      Live updates paused. Reconnecting…
    </div>
  );
}
