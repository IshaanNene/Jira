import { lazy } from "react";

export { dashboardKeys } from "./api";

export const DashboardsPage = lazy(() => import("./pages/DashboardsPage"));
export const DashboardPage = lazy(() => import("./pages/DashboardPage"));
