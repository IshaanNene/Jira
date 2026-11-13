import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useLiveChannel } from "@/modules/live";
import { api, type NotificationList, type UnreadCount } from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/notifications";

export const notificationKeys = {
  all: (orgId: string | undefined) => ["org", orgId, "notifications"] as const,
  list: (orgId: string | undefined, unread: boolean) => ["org", orgId, "notifications", "list", unread] as const,
  unread: (orgId: string | undefined) => ["org", orgId, "notifications", "unread"] as const,
};

const useOrgId = () => useSession((s) => s.organization?.id);

export function useNotifications(unread: boolean, enabled: boolean) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: notificationKeys.list(orgId, unread),
    queryFn: () => api.get<NotificationList>(BASE, { query: { limit: 30, unread: unread ? "true" : undefined } }),
    enabled: Boolean(orgId) && enabled,
  });
}

export function useUnreadCount() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: notificationKeys.unread(orgId),
    queryFn: () => api.get<UnreadCount>(`${BASE}/unread-count`),
    select: (d) => d.count,
    enabled: Boolean(orgId),
    // Live updates keep it current; this is the fallback when they are interrupted.
    refetchInterval: 120_000,
  });
}

export function useMarkRead() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const refresh = () => qc.invalidateQueries({ queryKey: notificationKeys.all(orgId) });
  return {
    one: useMutation({ mutationFn: (id: string) => api.post(`${BASE}/${id}/read`), onSettled: refresh }),
    all: useMutation({ mutationFn: () => api.post(`${BASE}/read-all`), onSettled: refresh }),
  };
}

/** New notifications arrive on the person's own channel while the app is open. */
export function useNotificationLive() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const userId = useSession((s) => s.account?.id);
  const refresh = () => void qc.invalidateQueries({ queryKey: notificationKeys.all(orgId) });
  return useLiveChannel(orgId && userId ? `user:${userId}` : undefined, refresh, refresh);
}
