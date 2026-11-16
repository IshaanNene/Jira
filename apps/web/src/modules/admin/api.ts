import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  api,
  ApiError,
  type CreatedProvisioningToken,
  type MemberTwoStepList,
  type ProvisionedGroup,
  type ProvisionedGroupList,
  type ProvisionedUserList,
  type ProvisioningTokenList,
  type SaveSsoConnection,
  type SsoConnection,
  type WorkspaceApiTokenList,
} from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/auth/admin";
const useOrgId = () => useSession((s) => s.organization?.id);
const keys = {
  twoStep: (orgId: string | undefined, ids: string[]) => ["org", orgId, "admin", "two-step", ids] as const,
  tokens: (orgId: string | undefined) => ["org", orgId, "admin", "api-tokens"] as const,
  sso: (orgId: string | undefined) => ["org", orgId, "admin", "sso"] as const,
  provisioning: (orgId: string | undefined) => ["org", orgId, "admin", "provisioning"] as const,
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

/** The workspace's identity provider, or `null` when single sign-on is not set up. */
export function useSsoConnection() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.sso(orgId),
    queryFn: async () => {
      try {
        return await api.get<SsoConnection>("/api/v1/auth/sso/connection");
      } catch (error) {
        if (error instanceof ApiError && error.status === 404) return null;
        throw error;
      }
    },
    enabled: Boolean(orgId),
  });
}

export function useSsoConnectionMutations() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return {
    save: useMutation({
      mutationFn: (input: SaveSsoConnection) => api.put<SsoConnection>("/api/v1/auth/sso/connection", input),
      onSuccess: (saved) => qc.setQueryData(keys.sso(orgId), saved),
    }),
    remove: useMutation({
      mutationFn: () => api.delete("/api/v1/auth/sso/connection"),
      onSuccess: () => qc.setQueryData(keys.sso(orgId), null),
    }),
  };
}

const PROVISIONING = "/api/v1/organizations/current/provisioning";

/** The workspace's SCIM provisioning tokens and the base URL to give the identity provider. */
export function useProvisioningTokens() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: [...keys.provisioning(orgId), "tokens"],
    queryFn: () => api.get<ProvisioningTokenList>(`${PROVISIONING}/tokens`),
    enabled: Boolean(orgId),
  });
}

/** People the identity provider has provisioned, and where each stands in the workspace. */
export function useProvisionedUsers() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: [...keys.provisioning(orgId), "users"],
    queryFn: () => api.get<ProvisionedUserList>(`${PROVISIONING}/users`),
    enabled: Boolean(orgId),
  });
}

/** The identity provider's groups, and the role each gives its members. */
export function useProvisionedGroups() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: [...keys.provisioning(orgId), "groups"],
    queryFn: () => api.get<ProvisionedGroupList>(`${PROVISIONING}/groups`),
    enabled: Boolean(orgId),
  });
}

/** Give a group a role (or none); its members' roles change at once. */
export function useGroupRoleMutation() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: ({ id, role }: { id: string; role: string | null }) =>
      api.put<ProvisionedGroup>(`${PROVISIONING}/groups/${id}/role`, { org_role: role }),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: keys.provisioning(orgId) });
      void qc.invalidateQueries({ queryKey: ["org", orgId, "members"] });
    },
  });
}

export function useProvisioningTokenMutations() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const refresh = () => qc.invalidateQueries({ queryKey: keys.provisioning(orgId) });
  return {
    create: useMutation({
      mutationFn: (name: string) => api.post<CreatedProvisioningToken>(`${PROVISIONING}/tokens`, { name }),
      onSettled: refresh,
    }),
    revoke: useMutation({
      mutationFn: (id: string) => api.delete(`${PROVISIONING}/tokens/${id}`),
      onSettled: refresh,
    }),
  };
}
