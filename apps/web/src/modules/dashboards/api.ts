import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  api,
  ApiError,
  requestWithMeta,
  type CreateDashboard,
  type CreateWidget,
  type Dashboard,
  type DashboardList,
  type UpdateDashboard,
  type UpdateWidget,
  type Widget,
  type WidgetData,
  type WidgetQuery,
} from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/dashboards";

const useOrgId = () => useSession((s) => s.organization?.id);

export const dashboardKeys = {
  all: (orgId: string | undefined) => ["org", orgId, "dashboards"] as const,
  list: (orgId: string | undefined) => [...dashboardKeys.all(orgId), "list"] as const,
  one: (orgId: string | undefined, id: string) => [...dashboardKeys.all(orgId), "one", id] as const,
  data: (orgId: string | undefined, query: WidgetQuery) => ["org", orgId, "analytics", query] as const,
};

export function useDashboards() {
  const orgId = useOrgId();
  return useQuery({
    queryKey: dashboardKeys.list(orgId),
    queryFn: () => api.get<DashboardList>(BASE),
    enabled: Boolean(orgId),
    select: (d) => d.items,
  });
}

export function useDashboard(id: string) {
  const orgId = useOrgId();
  return useQuery({ queryKey: dashboardKeys.one(orgId, id), queryFn: () => api.get<Dashboard>(`${BASE}/${id}`), enabled: Boolean(orgId && id) });
}

/**
 * The numbers behind one chart. Charts follow events, so a minute-old answer is fine;
 * `refreshMs` asks again on that interval while the chart is on screen.
 */
export function useWidgetData(query: WidgetQuery | null, refreshMs?: number) {
  const orgId = useOrgId();
  return useQuery({
    refetchInterval: refreshMs || false,
    queryKey: dashboardKeys.data(orgId, query ?? { kind: "velocity", project: "", sprints: 0 }),
    queryFn: ({ signal }) => api.post<WidgetData>("/api/v1/analytics/query", query, { signal }),
    enabled: Boolean(orgId && query),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  });
}

/**
 * A change to a dashboard or one of its charts, based on the version on screen (sent as
 * `If-Match`). A dashboard answer replaces the cached copy; other answers move its version on
 * and refetch it. If someone else changed it first, the cache takes their version (from the 409)
 * and the error is passed on, so nothing is overwritten unseen.
 */
function useDashboardMutation<Vars, Result>(id: string, run: (vars: Vars, version: number | undefined) => Promise<{ data: Result; version: number | undefined }>) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const key = dashboardKeys.one(orgId, id);
  return useMutation({
    mutationFn: async (vars: Vars) => {
      const { data, version } = await run(vars, qc.getQueryData<Dashboard>(key)?.version);
      // Right away, so a second change made before the refetch lands is based on this one.
      if (version !== undefined) qc.setQueryData<Dashboard>(key, (d) => (d ? { ...d, version } : d));
      return data;
    },
    onSuccess: (data) => {
      if (data && typeof data === "object" && "widgets" in data) qc.setQueryData(key, data);
      else void qc.invalidateQueries({ queryKey: key });
      void qc.invalidateQueries({ queryKey: dashboardKeys.list(orgId) });
    },
    onError: (error) => {
      if (error instanceof ApiError && error.isConflict && error.current) qc.setQueryData(key, error.current as Dashboard);
    },
  });
}

export function useCreateDashboard() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (body: CreateDashboard) => api.post<Dashboard>(BASE, body),
    onSuccess: (dashboard) => {
      qc.setQueryData(dashboardKeys.one(orgId, dashboard.id), dashboard);
      void qc.invalidateQueries({ queryKey: dashboardKeys.list(orgId) });
    },
  });
}

/** A private copy of a dashboard the caller can see, with its charts, owned by them. */
export function useCopyDashboard() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (id: string) => api.post<Dashboard>(`${BASE}/${id}/copy`),
    onSuccess: (dashboard) => {
      qc.setQueryData(dashboardKeys.one(orgId, dashboard.id), dashboard);
      void qc.invalidateQueries({ queryKey: dashboardKeys.list(orgId) });
    },
  });
}

export function useUpdateDashboard(id: string) {
  return useDashboardMutation(id, (body: UpdateDashboard, ifMatch) => requestWithMeta<Dashboard>("PATCH", `${BASE}/${id}`, { body, ifMatch }));
}

export function useDeleteDashboard() {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: (id: string) => api.delete(`${BASE}/${id}`),
    onSuccess: (_, id) => {
      qc.removeQueries({ queryKey: dashboardKeys.one(orgId, id) });
      void qc.invalidateQueries({ queryKey: dashboardKeys.list(orgId) });
    },
  });
}

export function useAddWidget(dashboardId: string) {
  return useDashboardMutation(dashboardId, (body: CreateWidget, ifMatch) =>
    requestWithMeta<Widget>("POST", `${BASE}/${dashboardId}/widgets`, { body, ifMatch }),
  );
}

export function useUpdateWidget(dashboardId: string) {
  return useDashboardMutation(dashboardId, ({ widgetId, ...body }: UpdateWidget & { widgetId: string }, ifMatch) =>
    requestWithMeta<Widget>("PATCH", `${BASE}/${dashboardId}/widgets/${widgetId}`, { body, ifMatch }),
  );
}

export function useRemoveWidget(dashboardId: string) {
  return useDashboardMutation(dashboardId, (widgetId: string, ifMatch) =>
    requestWithMeta<void>("DELETE", `${BASE}/${dashboardId}/widgets/${widgetId}`, { ifMatch }),
  );
}
