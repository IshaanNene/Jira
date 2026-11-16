import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, type Webhook, type WebhookDeliveryList, type WebhookEventTypeList, type WebhookList, type WebhookWithSecret } from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/webhooks";
const useOrgId = () => useSession((s) => s.organization?.id);
const keys = {
  all: (orgId: string | undefined) => ["org", orgId, "webhooks"] as const,
  deliveries: (orgId: string | undefined, id: string) => ["org", orgId, "webhooks", id, "deliveries"] as const,
};

export function useWebhooks() {
  const orgId = useOrgId();
  return useQuery({ queryKey: keys.all(orgId), queryFn: () => api.get<WebhookList>(BASE), select: (d) => d.items, enabled: Boolean(orgId) });
}

export function useWebhookEvents() {
  return useQuery({ queryKey: ["webhook-events"], queryFn: () => api.get<WebhookEventTypeList>(`${BASE}/events`), select: (d) => d.items, staleTime: Infinity });
}

export function useWebhookDeliveries(id: string | null) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.deliveries(orgId, id ?? ""),
    queryFn: () => api.get<WebhookDeliveryList>(`${BASE}/${id}/deliveries`),
    select: (d) => d.items,
    enabled: Boolean(orgId && id),
    // Deliveries happen in the background; keep the log fresh while it is open.
    refetchInterval: 5000,
  });
}

export interface WebhookInput {
  name: string;
  url: string;
  events: string[];
  project_ids: string[];
}

export function useWebhookMutations() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const refresh = () => qc.invalidateQueries({ queryKey: keys.all(orgId) });
  return {
    create: useMutation({ mutationFn: (input: WebhookInput) => api.post<WebhookWithSecret>(BASE, input), onSettled: refresh }),
    update: useMutation({
      mutationFn: ({ id, ...patch }: Partial<WebhookInput> & { id: string; enabled?: boolean }) => api.patch<Webhook>(`${BASE}/${id}`, patch),
      onSettled: refresh,
    }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BASE}/${id}`), onSettled: refresh }),
    rotate: useMutation({ mutationFn: (id: string) => api.post<WebhookWithSecret>(`${BASE}/${id}/secret`), onSettled: refresh }),
    test: useMutation({
      mutationFn: (id: string) => api.post(`${BASE}/${id}/test`),
      onSettled: (_d, _e, id) => qc.invalidateQueries({ queryKey: keys.deliveries(orgId, id) }),
    }),
  };
}
