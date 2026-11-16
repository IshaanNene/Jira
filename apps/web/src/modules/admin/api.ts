import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, type MemberTwoStepList, type WorkspaceApiTokenList } from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/auth/admin";
const useOrgId = () => useSession((s) => s.organization?.id);
const keys = {
  twoStep: (orgId: string | undefined, ids: string[]) => ["org", orgId, "admin", "two-step", ids] as const,
  tokens: (orgId: string | undefined) => ["org", orgId, "admin", "api-tokens"] as const,
};

/** Which of these members have two-step verification on. The server answers only for active members. */
export function useMemberTwoStep(userIds: string[]) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.twoStep(orgId, userIds),
    queryFn: async () => {
      const enabled = new Map<string, boolean>();
      // The server takes at most 500 ids per request.
      for (let i = 0; i < userIds.length; i += 500) {
        const page = await api.post<MemberTwoStepList>(`${BASE}/two-step`, { user_ids: userIds.slice(i, i + 500) });
        for (const item of page.items) enabled.set(item.user_id, item.enabled);
      }
      return enabled;
    },
    enabled: Boolean(orgId) && userIds.length > 0,
  });
}

export function useWorkspaceApiTokens() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.tokens(orgId),
    queryFn: () => api.get<WorkspaceApiTokenList>(`${BASE}/api-tokens`),
    select: (d) => d.items,
    enabled: Boolean(orgId),
  });
}

export function useRevokeWorkspaceApiToken() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (id: string) => api.delete(`${BASE}/api-tokens/${id}`),
    onSettled: () => qc.invalidateQueries({ queryKey: keys.tokens(orgId) }),
  });
}
