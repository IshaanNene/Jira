import { useInfiniteQuery, useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";

import {
  api,
  ApiError,
  requestWithMeta,
  type Attachment,
  type AttachmentList,
  type CommentList,
  type CustomField,
  type CustomFieldList,
  type HistoryList,
  type Issue,
  type IssueComment,
  type IssueDetail,
  type IssueMeta,
  type IssuePage,
  type LinkList,
  type RichDoc,
  type TransitionList,
  type WorkflowStatusList,
} from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/issues";

export interface IssueFilters {
  q?: string;
  status?: string;
  /** Comma-separated: todo, in_progress, done. */
  status_category?: string;
  type?: string;
  assignee?: string;
  parent?: string;
}

export const issueKeys = {
  all: (orgId: string | undefined) => ["org", orgId, "issues"] as const,
  lists: (orgId: string | undefined, project: string) => ["org", orgId, "issues", "list", project] as const,
  list: (orgId: string | undefined, project: string, filters: IssueFilters) => ["org", orgId, "issues", "list", project, filters] as const,
  one: (orgId: string | undefined, key: string) => ["org", orgId, "issues", "one", key] as const,
  sub: (orgId: string | undefined, key: string, what: string) => ["org", orgId, "issues", "one", key, what] as const,
};

const useOrgId = () => useSession((s) => s.organization?.id);

export function useIssueMeta() {
  return useQuery({ queryKey: ["issue-meta"], queryFn: () => api.get<IssueMeta>(`${BASE}/meta`), staleTime: Infinity });
}

export function useIssues(projectKey: string, filters: IssueFilters = {}, enabled = true) {
  const orgId = useOrgId();
  return useInfiniteQuery({
    queryKey: issueKeys.list(orgId, projectKey, filters),
    queryFn: ({ pageParam, signal }) => api.get<IssuePage>(BASE, { query: { project: projectKey, limit: 50, cursor: pageParam, ...filters }, signal }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: Boolean(orgId && projectKey) && enabled,
  });
}

export function useIssue(key: string) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: issueKeys.one(orgId, key),
    queryFn: () => api.get<IssueDetail>(`${BASE}/${key}`),
    enabled: Boolean(orgId && key),
    retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2,
  });
}

export interface NewIssue {
  project_key: string;
  type: string;
  summary: string;
  description?: RichDoc | null;
  priority?: string;
  assignee_id?: string | null;
  parent_key?: string | null;
  labels?: string[];
  custom_fields?: Record<string, unknown>;
}

export function useCreateIssue() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (input: NewIssue) => api.post<Issue>(BASE, input),
    onSuccess: (issue) => qc.invalidateQueries({ queryKey: issueKeys.lists(orgId, issue.project_key) }),
  });
}

/** Fields that can be edited inline. `null` clears a nullable field. */
export interface IssuePatch {
  summary?: string;
  description?: RichDoc | null;
  type?: string;
  status?: string;
  priority?: string;
  assignee_id?: string | null;
  parent_key?: string | null;
  labels?: string[];
  component_ids?: string[];
  fix_version_ids?: string[];
  /** Only the listed fields change; `null` clears one. */
  custom_fields?: Record<string, unknown>;
  story_points?: number | null;
  start_date?: string | null;
  due_date?: string | null;
}

function refreshLists(qc: QueryClient, orgId: string | undefined, projectKey: string) {
  void qc.invalidateQueries({ queryKey: issueKeys.lists(orgId, projectKey) });
}

/**
 * Inline edit with optimistic update and safe conflict handling:
 *  - the cache is patched immediately so the control feels instant;
 *  - the request carries the version the edit was based on (If-Match);
 *  - on a conflict the server's current issue replaces the cache — nothing is overwritten
 *    silently, and the caller is told so it can explain what happened.
 */
export function useUpdateIssue(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const queryKey = issueKeys.one(orgId, key);
  return useMutation({
    mutationFn: async (patch: IssuePatch) => {
      const cached = qc.getQueryData<IssueDetail>(queryKey);
      if (!cached) throw new Error("issue not loaded");
      return (await requestWithMeta<Issue>("PATCH", `${BASE}/${key}`, { body: patch, ifMatch: cached.version })).data;
    },
    onMutate: async (patch) => {
      await qc.cancelQueries({ queryKey });
      const previous = qc.getQueryData<IssueDetail>(queryKey);
      if (previous) {
        // parent_key is not a field of the cached issue; the server response fills in `parent`.
        const direct: IssuePatch = { ...patch };
        delete direct.parent_key;
        // A status change is decided by the workflow engine and may be refused, so the
        // view keeps the current status until the server answers.
        delete direct.status;
        if (patch.custom_fields) {
          // A patch lists only changed fields; merge them into what is cached.
          const merged: Record<string, unknown> = { ...(previous.custom_fields as Record<string, unknown>) };
          for (const [id, value] of Object.entries(patch.custom_fields)) {
            if (value === null) delete merged[id];
            else merged[id] = value;
          }
          direct.custom_fields = merged;
        }
        // The version is NOT bumped optimistically: it must stay the server's.
        qc.setQueryData<IssueDetail>(queryKey, { ...previous, ...(direct as Partial<IssueDetail>) });
      }
      return { previous };
    },
    onSuccess: (issue) => {
      qc.setQueryData<IssueDetail>(queryKey, (old) => (old ? { ...old, ...issue } : old));
      refreshLists(qc, orgId, issue.project_key);
      void qc.invalidateQueries({ queryKey: issueKeys.sub(orgId, key, "history") });
      // Which steps are available depends on the status and on fields like the assignee.
      void qc.invalidateQueries({ queryKey: issueKeys.sub(orgId, key, "transitions") });
    },
    onError: (error, _patch, context) => {
      if (error instanceof ApiError && error.isConflict && error.current) {
        const current = error.current as Issue;
        qc.setQueryData<IssueDetail>(queryKey, (old) => (old ? { ...old, ...current } : old));
      } else if (context?.previous) {
        qc.setQueryData(queryKey, context.previous);
      }
    },
  });
}

export function useDeleteIssue(key: string, projectKey: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: () => api.delete(`${BASE}/${key}`),
    onSuccess: () => {
      qc.removeQueries({ queryKey: issueKeys.one(orgId, key) });
      refreshLists(qc, orgId, projectKey);
    },
  });
}

export function useWatch(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (watch: boolean) => (watch ? api.put(`${BASE}/${key}/watch`) : api.delete(`${BASE}/${key}/watch`)),
    onMutate: (watch) => {
      qc.setQueryData<IssueDetail>(issueKeys.one(orgId, key), (old) =>
        old && old.watching !== watch ? { ...old, watching: watch, watcher_count: old.watcher_count + (watch ? 1 : -1) } : old,
      );
    },
    onSettled: () => qc.invalidateQueries({ queryKey: issueKeys.one(orgId, key) }),
  });
}

export function useComments(key: string) {
  const orgId = useOrgId();
  return useQuery({ queryKey: issueKeys.sub(orgId, key, "comments"), queryFn: () => api.get<CommentList>(`${BASE}/${key}/comments`), select: (d) => d.items });
}

export function useCommentMutations(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => qc.invalidateQueries({ queryKey: issueKeys.sub(orgId, key, "comments") });
  return {
    add: useMutation({ mutationFn: (body: RichDoc) => api.post<IssueComment>(`${BASE}/${key}/comments`, { body }), onSuccess: done }),
    edit: useMutation({ mutationFn: ({ id, body }: { id: string; body: RichDoc }) => api.patch<IssueComment>(`${BASE}/${key}/comments/${id}`, { body }), onSuccess: done }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BASE}/${key}/comments/${id}`), onSettled: done }),
  };
}

export function useLinks(key: string) {
  const orgId = useOrgId();
  return useQuery({ queryKey: issueKeys.sub(orgId, key, "links"), queryFn: () => api.get<LinkList>(`${BASE}/${key}/links`), select: (d) => d.items });
}

export function useLinkMutations(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  // A link shows on both issues, so every issue's link list may be stale.
  const done = () => qc.invalidateQueries({ queryKey: ["org", orgId, "issues", "one"], predicate: (q) => q.queryKey[q.queryKey.length - 1] === "links" });
  return {
    add: useMutation({ mutationFn: (input: { link_type: string; target_key: string }) => api.post(`${BASE}/${key}/links`, input), onSuccess: done }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BASE}/${key}/links/${id}`), onSettled: done }),
  };
}

export function useHistory(key: string, enabled: boolean) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: issueKeys.sub(orgId, key, "history"),
    queryFn: () => api.get<HistoryList>(`${BASE}/${key}/history`),
    select: (d) => d.items,
    enabled,
  });
}

// --- custom fields ----------------------------------------------------------

const FIELDS = "/api/v1/custom-fields";
const fieldsKey = (orgId: string | undefined) => ["org", orgId, "custom-fields"] as const;

export function useCustomFields() {
  const orgId = useOrgId();
  return useQuery({ queryKey: fieldsKey(orgId), queryFn: () => api.get<CustomFieldList>(FIELDS), select: (d) => d.items, enabled: Boolean(orgId), staleTime: 60_000 });
}

/** The fields that are part of issues of `issueType` in `projectKey`. */
export function applicableFields(fields: CustomField[] | undefined, projectKey: string, issueType: string): CustomField[] {
  return (fields ?? []).filter(
    (f) => (f.project_keys.length === 0 || f.project_keys.includes(projectKey)) && (f.issue_types.length === 0 || f.issue_types.includes(issueType)),
  );
}

export interface CustomFieldInput {
  name: string;
  field_type: string;
  options: string[];
  required: boolean;
  project_keys: string[];
  issue_types: string[];
}

export function useCustomFieldMutations() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => qc.invalidateQueries({ queryKey: fieldsKey(orgId) });
  return {
    create: useMutation({ mutationFn: (input: CustomFieldInput) => api.post<CustomField>(FIELDS, input), onSuccess: done }),
    update: useMutation({
      mutationFn: ({ id, field_type: _type, ...input }: CustomFieldInput & { id: string }) => api.patch<CustomField>(`${FIELDS}/${id}`, input),
      onSuccess: done,
    }),
    archive: useMutation({ mutationFn: (id: string) => api.delete(`${FIELDS}/${id}`), onSettled: done }),
  };
}

// --- attachments ------------------------------------------------------------

export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

export function useAttachments(key: string) {
  const orgId = useOrgId();
  return useQuery({ queryKey: issueKeys.sub(orgId, key, "attachments"), queryFn: () => api.get<AttachmentList>(`${BASE}/${key}/attachments`), select: (d) => d.items });
}

export function useAttachmentMutations(key: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => {
    void qc.invalidateQueries({ queryKey: issueKeys.sub(orgId, key, "attachments") });
    void qc.invalidateQueries({ queryKey: issueKeys.sub(orgId, key, "history") });
  };
  return {
    upload: useMutation({
      mutationFn: (file: File) => {
        const form = new FormData();
        form.append("file", file);
        return api.upload<Attachment>(`${BASE}/${key}/attachments`, form);
      },
      onSuccess: done,
    }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BASE}/${key}/attachments/${id}`), onSettled: done }),
  };
}

export const attachmentContentPath = (key: string, id: string) => `${BASE}/${key}/attachments/${id}/content`;

// --- workflow ---------------------------------------------------------------

/** Status changes the current user may make on this issue, as decided by its workflow. */
export function useTransitions(key: string, enabled: boolean) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: issueKeys.sub(orgId, key, "transitions"),
    queryFn: () => api.get<TransitionList>(`${BASE}/${key}/transitions`),
    select: (d) => d.items,
    enabled: Boolean(orgId) && enabled,
  });
}

/** Every status an issue of the project can be in (across its workflows), for filters. */
export function useProjectStatuses(projectId: string | undefined) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: ["org", orgId, "workflow-statuses", projectId],
    queryFn: () => api.get<WorkflowStatusList>("/api/v1/workflow-statuses", { query: { project_id: projectId } }),
    select: (d) => d.items,
    enabled: Boolean(orgId && projectId),
    staleTime: 60_000,
  });
}
