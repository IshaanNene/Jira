import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import {
  api,
  ApiError,
  type CreateSavedFilter,
  type FilterSubscription,
  type MatchingIds,
  type PutFilterSubscription,
  type SavedFilter,
  type SavedFilterList,
  type SearchResults,
  type UpdateSavedFilter,
} from "@/services/api";
import { useSession } from "@/state/session";

const PATH = "/api/v1/search/issues";
const MQL_PATH = "/api/v1/search/issues/query";
const FILTERS_PATH = "/api/v1/search/filters";
const IDS_PATH = "/api/v1/search/issues/ids";

export interface SearchParams {
  q: string;
  project_id?: string;
  status_category?: string;
  assignee?: string;
}

export const searchKeys = {
  all: (orgId: string | undefined) => ["org", orgId, "search"] as const,
  query: (orgId: string | undefined, params: SearchParams, limit: number) => ["org", orgId, "search", params, limit] as const,
  mql: (orgId: string | undefined, mql: string) => ["org", orgId, "search", "mql", mql] as const,
  ids: (orgId: string | undefined, projectId: string, mql: string) => ["org", orgId, "search", "ids", projectId, mql] as const,
  filters: (orgId: string | undefined) => ["org", orgId, "search-filters"] as const,
  filter: (orgId: string | undefined, id: string) => ["org", orgId, "search-filters", id] as const,
  subscription: (orgId: string | undefined, id: string) => ["org", orgId, "search-filters", id, "subscription"] as const,
};

/** `value`, once it has stopped changing for `ms`. */
export function useDebounced<T>(value: T, ms = 200): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return settled;
}

/** A handful of top matches (typeahead). */
export function useQuickSearch(q: string, enabled: boolean) {
  const orgId = useSession((s) => s.organization?.id);
  const params = { q: q.trim() };
  return useQuery({
    queryKey: searchKeys.query(orgId, params, 6),
    queryFn: ({ signal }) => api.get<SearchResults>(PATH, { query: { q: params.q, limit: 6 }, signal }),
    enabled: Boolean(orgId) && enabled && params.q.length >= 2,
    placeholderData: keepPreviousData,
    staleTime: 15_000,
  });
}

/** Full results, a page at a time. */
export function useSearch(params: SearchParams) {
  const orgId = useSession((s) => s.organization?.id);
  return useInfiniteQuery({
    queryKey: searchKeys.query(orgId, params, 25),
    queryFn: ({ pageParam, signal }) => api.get<SearchResults>(PATH, { query: { ...params, limit: 25, cursor: pageParam }, signal }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: Boolean(orgId),
    placeholderData: keepPreviousData,
  });
}

/** Results of an MQL query, a page at a time. Invalid queries fail with a 422 whose field
 * error carries the span at fault; those are not retried. */
export function useMqlSearch(mql: string, enabled = true) {
  const orgId = useSession((s) => s.organization?.id);
  return useInfiniteQuery({
    queryKey: searchKeys.mql(orgId, mql),
    queryFn: ({ pageParam, signal }) => api.get<SearchResults>(MQL_PATH, { query: { mql, limit: 25, cursor: pageParam }, signal }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: Boolean(orgId) && enabled,
    retry: (count, error) => !(error instanceof ApiError && error.status === 422) && count < 2,
  });
}

/** Ids of the project's issues that match an MQL query, for filtering a board or backlog.
 * Nothing is fetched for an empty query. */
export function useMatchingIds(projectId: string, mql: string) {
  const orgId = useSession((s) => s.organization?.id);
  return useQuery({
    queryKey: searchKeys.ids(orgId, projectId, mql),
    queryFn: ({ signal }) => api.get<MatchingIds>(IDS_PATH, { query: { mql, project: projectId }, signal }),
    enabled: Boolean(orgId) && mql.trim() !== "",
    retry: (count, error) => !(error instanceof ApiError && error.status === 422) && count < 2,
    placeholderData: keepPreviousData,
  });
}

/** The person's own saved filters, then the ones others shared. */
export function useSavedFilters() {
  const orgId = useSession((s) => s.organization?.id);
  return useQuery({
    queryKey: searchKeys.filters(orgId),
    queryFn: ({ signal }) => api.get<SavedFilterList>(FILTERS_PATH, { signal }),
    enabled: Boolean(orgId),
  });
}

export function useSavedFilter(id: string | null) {
  const orgId = useSession((s) => s.organization?.id);
  return useQuery({
    queryKey: searchKeys.filter(orgId, id ?? ""),
    queryFn: ({ signal }) => api.get<SavedFilter>(`${FILTERS_PATH}/${id}`, { signal }),
    enabled: Boolean(orgId) && Boolean(id),
    retry: (count, error) => !(error instanceof ApiError && error.status === 404) && count < 2,
  });
}

function useRefreshFilters() {
  const orgId = useSession((s) => s.organization?.id);
  const queryClient = useQueryClient();
  return () => queryClient.invalidateQueries({ queryKey: searchKeys.filters(orgId) });
}

export function useCreateSavedFilter() {
  const refresh = useRefreshFilters();
  return useMutation({ mutationFn: (input: CreateSavedFilter) => api.post<SavedFilter>(FILTERS_PATH, input), onSettled: refresh });
}

export function useUpdateSavedFilter() {
  const refresh = useRefreshFilters();
  return useMutation({
    mutationFn: ({ id, ...input }: UpdateSavedFilter & { id: string }) => api.patch<SavedFilter>(`${FILTERS_PATH}/${id}`, input),
    onSettled: refresh,
  });
}

export function useDeleteSavedFilter() {
  const refresh = useRefreshFilters();
  return useMutation({ mutationFn: (id: string) => api.delete(`${FILTERS_PATH}/${id}`), onSettled: refresh });
}

/** The caller's email subscription to a saved filter; `null` when there is none. */
export function useFilterSubscription(filterId: string) {
  const orgId = useSession((s) => s.organization?.id);
  return useQuery({
    queryKey: searchKeys.subscription(orgId, filterId),
    queryFn: async ({ signal }) => {
      try {
        return await api.get<FilterSubscription>(`${FILTERS_PATH}/${filterId}/subscription`, { signal });
      } catch (error) {
        if (error instanceof ApiError && error.status === 404) return null;
        throw error;
      }
    },
    enabled: Boolean(orgId),
  });
}

export function useSaveFilterSubscription(filterId: string) {
  const orgId = useSession((s) => s.organization?.id);
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: PutFilterSubscription) => api.put<FilterSubscription>(`${FILTERS_PATH}/${filterId}/subscription`, input),
    onSuccess: (sub) => queryClient.setQueryData(searchKeys.subscription(orgId, filterId), sub),
  });
}

export function useDeleteFilterSubscription(filterId: string) {
  const orgId = useSession((s) => s.organization?.id);
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.delete(`${FILTERS_PATH}/${filterId}/subscription`),
    onSuccess: () => queryClient.setQueryData(searchKeys.subscription(orgId, filterId), null),
  });
}
