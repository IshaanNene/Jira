import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  api,
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

/** The numbers behind one chart. Charts follow events, so a minute-old answer is fine. */
export function useWidgetData(query: WidgetQuery | null) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: dashboardKeys.data(orgId, query ?? { kind: "velocity", project: "", sprints: 0 }),
    queryFn: ({ signal }) => api.post<WidgetData>("/api/v1/analytics/query", query, { signal }),
    enabled: Boolean(orgId && query),
    staleTime: 60_000,
    placeholderData: keepPreviousData,
  });
}

/** A dashboard answer replaces the cached copy; other answers refetch it. */
function useDashboardMutation<Vars, Result>(id: string, run: (vars: Vars) => Promise<Result>) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  return useMutation({
    mutationFn: run,
    onSuccess: (result) => {
      if (result && typeof result === "object" && "widgets" in result) qc.setQueryData(dashboardKeys.one(orgId, id), result);
      else void qc.invalidateQueries({ queryKey: dashboardKeys.one(orgId, id) });
      void qc.invalidateQueries({ queryKey: dashboardKeys.list(orgId) });
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

export function useUpdateDashboard(id: string) {
  return useDashboardMutation(id, (body: UpdateDashboard) => api.patch<Dashboard>(`${BASE}/${id}`, body));
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
  return useDashboardMutation(dashboardId, (body: CreateWidget) => api.post<Widget>(`${BASE}/${dashboardId}/widgets`, body));
}

export function useUpdateWidget(dashboardId: string) {
  return useDashboardMutation(dashboardId, ({ widgetId, ...body }: UpdateWidget & { widgetId: string }) =>
    api.patch<Widget>(`${BASE}/${dashboardId}/widgets/${widgetId}`, body),
  );
}

export function useRemoveWidget(dashboardId: string) {
  return useDashboardMutation(dashboardId, (widgetId: string) => api.delete(`${BASE}/${dashboardId}/widgets/${widgetId}`));
}
