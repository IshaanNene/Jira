import { keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { api, type SearchResults } from "@/services/api";
import { useSession } from "@/state/session";

const PATH = "/api/v1/search/issues";

export interface SearchParams {
  q: string;
  project_id?: string;
  status_category?: string;
  assignee?: string;
}

export const searchKeys = {
  all: (orgId: string | undefined) => ["org", orgId, "search"] as const,
  query: (orgId: string | undefined, params: SearchParams, limit: number) => ["org", orgId, "search", params, limit] as const,
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
