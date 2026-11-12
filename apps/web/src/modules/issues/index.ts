import { lazy } from "react";

export { issueKeys, useIssueMeta, useIssues, useProjectStatuses } from "./api";
export { CreateIssueDialog } from "./components/CreateIssueDialog";
export { IssueTypeIcon, PriorityIcon, StatusLozenge } from "./components/visuals";

export const IssueListPage = lazy(() => import("./pages/IssueListPage"));
export const IssuePage = lazy(() => import("./pages/IssuePage"));
export const CustomFieldsPage = lazy(() => import("./pages/CustomFieldsPage"));
