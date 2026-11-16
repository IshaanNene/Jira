import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useLiveChannel } from "@/modules/live";
import { api, type NotificationList, type NotificationPreferences, type UnreadCount, type UnsubscribeLink } from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/notifications";

export const notificationKeys = {
  all: (orgId: string | undefined) => ["org", orgId, "notifications"] as const,
  list: (orgId: string | undefined, unread: boolean) => ["org", orgId, "notifications", "list", unread] as const,
  unread: (orgId: string | undefined) => ["org", orgId, "notifications", "unread"] as const,
  preferences: (orgId: string | undefined) => ["org", orgId, "notification-preferences"] as const,
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

export function usePreferences() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: notificationKeys.preferences(orgId),
    queryFn: () => api.get<NotificationPreferences>(`${BASE}/preferences`),
    enabled: Boolean(orgId),
  });
}

/** Saves at once; the switch moves immediately and moves back if saving fails. */
export function useSavePreferences() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const key = notificationKeys.preferences(orgId);
  return useMutation({
    mutationFn: (next: NotificationPreferences) => api.put<NotificationPreferences>(`${BASE}/preferences`, next),
    onMutate: async (next) => {
      await qc.cancelQueries({ queryKey: key });
      const previous = qc.getQueryData<NotificationPreferences>(key);
      qc.setQueryData(key, next);
      return { previous };
    },
    onError: (_error, _next, context) => qc.setQueryData(key, context?.previous),
    onSuccess: (saved) => qc.setQueryData(key, saved),
  });
}

/** Resume email to the person's own address after a bounce or spam complaint paused it. */
export function useResumeEmail() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const key = notificationKeys.preferences(orgId);
  return useMutation({
    mutationFn: () => api.delete(`${BASE}/preferences/email-pause`),
    onSuccess: () => {
      qc.setQueryData<NotificationPreferences>(key, (prev) => (prev ? { ...prev, email_paused: null } : prev));
      void qc.invalidateQueries({ queryKey: key });
    },
  });
}

/** What an emailed unsubscribe link would turn off. Works signed out. */
export function useUnsubscribeLink(token: string) {
  return useQuery({
    queryKey: ["unsubscribe", token],
    queryFn: () => api.get<UnsubscribeLink>(`${BASE}/unsubscribe`, { query: { token }, auth: false }),
    enabled: token.length > 0,
    retry: false,
  });
}

export function useUnsubscribe(token: string) {
  return useMutation({
    mutationFn: (all: boolean) => api.post(`${BASE}/unsubscribe`, all ? { all: true } : {}, { query: { token }, auth: false }),
  });
}
