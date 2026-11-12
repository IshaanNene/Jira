import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  api,
  requestWithMeta,
  type Component,
  type ComponentList,
  type CreateProjectRequest,
  type Project,
  type ProjectAccess,
  type ProjectList,
  type ProjectRole,
  type Version,
  type VersionList,
} from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/projects";

export const projectKeys = {
  all: (orgId: string | undefined) => ["org", orgId, "projects"] as const,
  list: (orgId: string | undefined, status: string) => ["org", orgId, "projects", "list", status] as const,
  one: (orgId: string | undefined, key: string) => ["org", orgId, "projects", "one", key] as const,
  sub: (orgId: string | undefined, key: string, what: string) => ["org", orgId, "projects", "one", key, what] as const,
};

const useOrgId = () => useSession((s) => s.organization?.id);

export function useProjects(status: "active" | "archived" = "active") {
  const orgId = useOrgId();
  return useQuery({
    queryKey: projectKeys.list(orgId, status),
    queryFn: () => api.get<ProjectList>(BASE, { query: { status } }),
    enabled: Boolean(orgId),
    select: (d) => d.items,
  });
}

export function useProject(key: string) {
  const orgId = useOrgId();
  return useQuery({ queryKey: projectKeys.one(orgId, key), queryFn: () => api.get<Project>(`${BASE}/${key}`), enabled: Boolean(orgId && key) });
}

export function useCreateProject() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (input: CreateProjectRequest) => api.post<Project>(BASE, input),
    onSuccess: () => qc.invalidateQueries({ queryKey: projectKeys.all(orgId) }),
  });
}

export interface ProjectPatch {
  name?: string;
  description?: string;
  project_type?: string;
  lead_user_id?: string | null;
}

export function useUpdateProject(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: async ({ patch, version }: { patch: ProjectPatch; version: number }) =>
      (await requestWithMeta<Project>("PATCH", `${BASE}/${key}`, { body: patch, ifMatch: version })).data,
    onSuccess: (project) => {
      qc.setQueryData(projectKeys.one(orgId, key), project);
      void qc.invalidateQueries({ queryKey: ["org", orgId, "projects", "list"] });
    },
  });
}

/** archive | restore | delete, plus the access default — all refresh every project query. */
export function useProjectAction(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => qc.invalidateQueries({ queryKey: projectKeys.all(orgId) });
  return {
    archive: useMutation({ mutationFn: () => api.post<Project>(`${BASE}/${key}/archive`), onSuccess: done }),
    restore: useMutation({ mutationFn: () => api.post<Project>(`${BASE}/${key}/restore`), onSuccess: done }),
    remove: useMutation({ mutationFn: () => api.delete(`${BASE}/${key}`), onSuccess: done }),
    setAccess: useMutation({
      mutationFn: (default_role: ProjectRole | null) => api.put<Project>(`${BASE}/${key}/access`, { default_role }),
      onSuccess: done,
    }),
  };
}

export function useProjectAccess(key: string) {
  const orgId = useOrgId();
  return useQuery({ queryKey: projectKeys.sub(orgId, key, "access"), queryFn: () => api.get<ProjectAccess>(`${BASE}/${key}/members`) });
}

export function useProjectMemberMutations(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => qc.invalidateQueries({ queryKey: projectKeys.sub(orgId, key, "access") });
  return {
    set: useMutation({ mutationFn: ({ userId, role }: { userId: string; role: ProjectRole }) => api.put(`${BASE}/${key}/members/${userId}`, { role }), onSettled: done }),
    remove: useMutation({ mutationFn: (userId: string) => api.delete(`${BASE}/${key}/members/${userId}`), onSettled: done }),
  };
}

export function useComponents(key: string) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: projectKeys.sub(orgId, key, "components"),
    queryFn: () => api.get<ComponentList>(`${BASE}/${key}/components`),
    select: (d) => d.items,
  });
}

export function useComponentMutations(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => qc.invalidateQueries({ queryKey: projectKeys.sub(orgId, key, "components") });
  return {
    create: useMutation({ mutationFn: (input: { name: string; description: string }) => api.post<Component>(`${BASE}/${key}/components`, input), onSuccess: done }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BASE}/${key}/components/${id}`), onSettled: done }),
  };
}

export function useVersions(key: string) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: projectKeys.sub(orgId, key, "versions"),
    queryFn: () => api.get<VersionList>(`${BASE}/${key}/versions`),
    select: (d) => d.items,
  });
}

export function useVersionMutations(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => qc.invalidateQueries({ queryKey: projectKeys.sub(orgId, key, "versions") });
  return {
    create: useMutation({
      mutationFn: (input: { name: string; release_date?: string | null }) => api.post<Version>(`${BASE}/${key}/versions`, input),
      onSuccess: done,
    }),
    setReleased: useMutation({
      mutationFn: ({ id, released }: { id: string; released: boolean }) => api.post<Version>(`${BASE}/${key}/versions/${id}/${released ? "release" : "unrelease"}`),
      onSettled: done,
    }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BASE}/${key}/versions/${id}`), onSettled: done }),
  };
}
