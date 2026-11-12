import { lazy } from "react";

export { projectKeys, useComponents, useProject, useProjects, useVersions } from "./api";
export { CreateProjectDialog } from "./components/CreateProjectDialog";
export { ProjectAvatar } from "./components/ProjectAvatar";
export { PROJECT_TYPE_LABEL, roleAtLeast } from "./roles";

export const ProjectsPage = lazy(() => import("./pages/ProjectsPage"));
export const ProjectLayout = lazy(() => import("./pages/ProjectLayout"));
export const ProjectSettingsPage = lazy(() => import("./pages/ProjectSettingsPage"));
