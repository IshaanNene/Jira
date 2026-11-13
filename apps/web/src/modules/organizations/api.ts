import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  api,
  download,
  requestWithMeta,
  type AcceptInvitationResponse,
  type AuditPage,
  type AuditVerification,
  type Invitation,
  type InvitationList,
  type InvitationPreview,
  type Member,
  type MemberPage,
  type Organization,
  type OrganizationList,
  type OrgRole,
} from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1";

/**
 * Every tenant-scoped key starts with the organization id, so data cached for one
 * organization can never be rendered for another.
 */
export const orgKeys = {
  mine: (userId: string | undefined) => ["organizations", "mine", userId] as const,
  current: (orgId: string | undefined) => ["org", orgId, "current"] as const,
  members: (orgId: string | undefined, search: string) => ["org", orgId, "members", search] as const,
  allMembers: (orgId: string | undefined) => ["org", orgId, "members"] as const,
  invitations: (orgId: string | undefined) => ["org", orgId, "invitations"] as const,
  invitationPreview: (token: string) => ["invitation-preview", token] as const,
  audit: (orgId: string | undefined, filters: AuditFilters) => ["org", orgId, "audit", filters] as const,
};

export interface AuditFilters {
  category?: string;
  actor?: string;
  from?: string;
  to?: string;
}

const AUDIT_PATH = `${BASE}/organizations/current/audit`;

function auditQuery(filters: AuditFilters): Record<string, string> {
  return Object.fromEntries(Object.entries(filters).filter(([, v]) => Boolean(v))) as Record<string, string>;
}

/** The workspace audit log, newest first, a page at a time (admins and owners). */
export function useAuditLog(filters: AuditFilters) {
  const orgId = useOrgId();
  return useInfiniteQuery({
    queryKey: orgKeys.audit(orgId, filters),
    queryFn: ({ pageParam, signal }) =>
      api.get<AuditPage>(AUDIT_PATH, { query: { ...auditQuery(filters), limit: 50, cursor: pageParam }, signal }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: Boolean(orgId),
  });
}

/** Re-check the whole hash chain on the server. */
export function useVerifyAuditLog() {
  return useMutation({ mutationFn: () => api.get<AuditVerification>(`${AUDIT_PATH}/verify`) });
}

/** The matching entries as CSV, fetched with the session's token. */
export function exportAuditLog(filters: AuditFilters): Promise<Blob> {
  const query = new URLSearchParams(auditQuery(filters)).toString();
  return download(`${AUDIT_PATH}/export${query ? `?${query}` : ""}`);
}

function useOrgId(): string | undefined {
  return useSession((s) => s.organization?.id);
}

export function useMyOrganizations() {
  const userId = useSession((s) => s.account?.id);
  return useQuery({
    queryKey: orgKeys.mine(userId),
    queryFn: () => api.get<OrganizationList>(`${BASE}/organizations`),
    enabled: Boolean(userId),
    select: (data) => data.items,
  });
}

export function useCreateOrganization() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; slug?: string }) => api.post<Organization>(`${BASE}/organizations`, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["organizations", "mine"] }),
  });
}

/** The current organization together with the version needed to edit it safely. */
export function useCurrentOrganization() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: orgKeys.current(orgId),
    queryFn: () => api.get<Organization>(`${BASE}/organizations/current`),
    enabled: Boolean(orgId),
  });
}

export function useRenameOrganization() {
  const queryClient = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: async ({ name, version }: { name: string; version: number }) =>
      (await requestWithMeta<Organization>("PATCH", `${BASE}/organizations/current`, { body: { name }, ifMatch: version })).data,
    onSuccess: (organization) => {
      queryClient.setQueryData(orgKeys.current(orgId), organization);
      void queryClient.invalidateQueries({ queryKey: ["organizations", "mine"] });
    },
  });
}

export function useMembers(search: string, enabled = true) {
  const orgId = useOrgId();
  return useInfiniteQuery({
    queryKey: orgKeys.members(orgId, search),
    queryFn: ({ pageParam, signal }) =>
      api.get<MemberPage>(`${BASE}/organizations/current/members`, { query: { limit: 50, cursor: pageParam, query: search }, signal }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: Boolean(orgId) && enabled,
  });
}

export function useUpdateMember() {
  const queryClient = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: ({ userId, ...change }: { userId: string; org_role?: OrgRole; status?: "active" | "suspended" }) =>
      api.patch<Member>(`${BASE}/organizations/current/members/${userId}`, change),
    onSettled: () => queryClient.invalidateQueries({ queryKey: orgKeys.allMembers(orgId) }),
  });
}

export function useRemoveMember() {
  const queryClient = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (userId: string) => api.delete(`${BASE}/organizations/current/members/${userId}`),
    onSettled: () => queryClient.invalidateQueries({ queryKey: orgKeys.allMembers(orgId) }),
  });
}

export function useInvitations(enabled: boolean) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: orgKeys.invitations(orgId),
    queryFn: () => api.get<InvitationList>(`${BASE}/organizations/current/invitations`),
    enabled: Boolean(orgId) && enabled,
    select: (data) => data.items,
  });
}

export function useCreateInvitation() {
  const queryClient = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (input: { email: string; org_role: OrgRole }) => api.post<Invitation>(`${BASE}/organizations/current/invitations`, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: orgKeys.invitations(orgId) }),
  });
}

export function useRevokeInvitation() {
  const queryClient = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (id: string) => api.delete(`${BASE}/organizations/current/invitations/${id}`),
    // Optimistic: the row disappears immediately and comes back if the server refuses.
    onMutate: async (id) => {
      const key = orgKeys.invitations(orgId);
      await queryClient.cancelQueries({ queryKey: key });
      const previous = queryClient.getQueryData<InvitationList>(key);
      if (previous) queryClient.setQueryData<InvitationList>(key, { items: previous.items.filter((i) => i.id !== id) });
      return { previous };
    },
    onError: (_error, _id, context) => {
      if (context?.previous) queryClient.setQueryData(orgKeys.invitations(orgId), context.previous);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: orgKeys.invitations(orgId) }),
  });
}

export function useInvitationPreview(token: string) {
  return useQuery({
    queryKey: orgKeys.invitationPreview(token),
    queryFn: () => api.get<InvitationPreview>(`${BASE}/invitations/${encodeURIComponent(token)}`, { auth: false }),
    retry: false,
    staleTime: 0,
  });
}

export function useAcceptInvitation() {
  return useMutation({
    mutationFn: (token: string) => api.post<AcceptInvitationResponse>(`${BASE}/invitations/${encodeURIComponent(token)}/accept`),
  });
}

/**
 * Everyone in the workspace, keyed by user id — used wherever another service's data
 * refers to a person by id (assignees, authors, project leads).
 */
export function useUserDirectory() {
  const orgId = useOrgId();
  const query = useQuery({
    queryKey: ["org", orgId, "members", "directory"],
    queryFn: async () => {
      const all: Member[] = [];
      let cursor: string | undefined;
      // Bounded: a directory picker past a few thousand people needs server-side search instead.
      for (let page = 0; page < 20; page += 1) {
        const result = await api.get<MemberPage>(`${BASE}/organizations/current/members`, { query: { limit: 200, cursor } });
        all.push(...result.items);
        if (!result.next_cursor) break;
        cursor = result.next_cursor;
      }
      return all;
    },
    enabled: Boolean(orgId),
    staleTime: 60_000,
  });
  const byId = new Map((query.data ?? []).map((m) => [m.user_id, m]));
  return { members: query.data ?? [], byId, isPending: query.isPending };
}
