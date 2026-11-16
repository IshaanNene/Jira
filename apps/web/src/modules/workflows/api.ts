import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  api,
  requestWithMeta,
  type CustomFieldList,
  type IssueMeta,
  type WorkflowAssignmentList,
  type WorkflowDefinition,
  type WorkflowDetail,
  type WorkflowList,
  type WorkflowVersionDetail,
  type WorkflowVersionList,
} from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/workflows";
const ASSIGNMENTS = "/api/v1/workflow-assignments";

const useOrgId = () => useSession((s) => s.organization?.id);
const keys = {
  all: (orgId: string | undefined) => ["org", orgId, "workflows"] as const,
  one: (orgId: string | undefined, id: string) => ["org", orgId, "workflows", id] as const,
  versions: (orgId: string | undefined, id: string) => ["org", orgId, "workflows", id, "versions"] as const,
  assignments: (orgId: string | undefined, projectId: string | null) => ["org", orgId, "workflows", "assignments", projectId] as const,
};

export function useWorkflows() {
  const orgId = useOrgId();
  return useQuery({ queryKey: keys.all(orgId), queryFn: () => api.get<WorkflowList>(BASE), select: (d) => d.items, enabled: Boolean(orgId) });
}

export function useWorkflow(id: string) {
  const orgId = useOrgId();
  return useQuery({ queryKey: keys.one(orgId, id), queryFn: () => api.get<WorkflowDetail>(`${BASE}/${id}`), enabled: Boolean(orgId && id) });
}

/** Published versions, newest first. */
export function useWorkflowVersions(id: string, enabled: boolean) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.versions(orgId, id),
    queryFn: () => api.get<WorkflowVersionList>(`${BASE}/${id}/versions`),
    select: (d) => d.items,
    enabled: Boolean(orgId) && enabled,
  });
}

export function useWorkflowVersion(id: string, version: number | null) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: [...keys.versions(orgId, id), version],
    queryFn: () => api.get<WorkflowVersionDetail>(`${BASE}/${id}/versions/${version}`),
    enabled: Boolean(orgId) && version !== null,
    staleTime: Infinity, // published versions never change
  });
}

export function useWorkflowMutations() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const refresh = () => qc.invalidateQueries({ queryKey: keys.all(orgId) });
  const store = (detail: WorkflowDetail) => {
    qc.setQueryData(keys.one(orgId, detail.id), detail);
    void qc.invalidateQueries({ queryKey: keys.all(orgId), exact: true });
    return detail;
  };
  const withVersion = async (method: string, path: string, version: number, body?: unknown) =>
    store((await requestWithMeta<WorkflowDetail>(method, path, { body, ifMatch: version })).data);
  return {
    create: useMutation({ mutationFn: (input: { name: string; copy_from?: string }) => api.post<WorkflowDetail>(BASE, input), onSuccess: refresh }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BASE}/${id}`), onSuccess: refresh }),
    saveDraft: useMutation({
      mutationFn: ({ id, version, definition }: { id: string; version: number; definition: WorkflowDefinition }) =>
        withVersion("PUT", `${BASE}/${id}/draft`, version, { definition }),
    }),
    discardDraft: useMutation({ mutationFn: ({ id, version }: { id: string; version: number }) => withVersion("DELETE", `${BASE}/${id}/draft`, version) }),
    publish: useMutation({
      mutationFn: ({ id, version }: { id: string; version: number }) => withVersion("POST", `${BASE}/${id}/publish`, version),
      onSuccess: (detail) => void qc.invalidateQueries({ queryKey: keys.versions(orgId, detail.id) }),
    }),
    restore: useMutation({
      mutationFn: ({ id, version, number }: { id: string; version: number; number: number }) =>
        withVersion("POST", `${BASE}/${id}/versions/${number}/restore`, version),
    }),
  };
}

/** Assignments for one project, or the workspace-wide defaults when `projectId` is null. */
export function useWorkflowAssignments(projectId: string | null) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.assignments(orgId, projectId),
    queryFn: () => api.get<WorkflowAssignmentList>(ASSIGNMENTS, { query: { project_id: projectId } }),
    select: (d) => d.items,
    enabled: Boolean(orgId),
  });
}

export function useAssignWorkflow(projectId: string | null) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: ({ issueType, workflowId }: { issueType: string | null; workflowId: string | null }) =>
      api.put(ASSIGNMENTS, { project_id: projectId, issue_type: issueType, workflow_id: workflowId }),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: keys.all(orgId) });
      // Statuses and available transitions of existing issues may have changed.
      void qc.invalidateQueries({ queryKey: ["org", orgId, "issues"] });
      void qc.invalidateQueries({ queryKey: ["org", orgId, "workflow-statuses"] });
    },
  });
}

// The designer needs issue types and custom fields as choices. They are fetched here
// with the same query keys the issues module uses, so the cache is shared without this
// module depending on that one.
export function useIssueTypes() {
  return useQuery({ queryKey: ["issue-meta"], queryFn: () => api.get<IssueMeta>("/api/v1/issues/meta"), staleTime: Infinity, select: (d) => d.types });
}

export function useCustomFieldChoices() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: ["org", orgId, "custom-fields"],
    queryFn: () => api.get<CustomFieldList>("/api/v1/custom-fields"),
    select: (d) => d.items,
    enabled: Boolean(orgId),
    staleTime: 60_000,
  });
}
