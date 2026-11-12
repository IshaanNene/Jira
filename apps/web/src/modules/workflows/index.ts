import { lazy } from "react";

export { ProjectWorkflowSettings } from "./components/ProjectWorkflowSettings";

export const WorkflowsPage = lazy(() => import("./pages/WorkflowsPage"));
export const WorkflowDesignerPage = lazy(() => import("./pages/WorkflowDesignerPage"));
