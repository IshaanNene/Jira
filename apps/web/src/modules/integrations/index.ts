import { lazy } from "react";

export const WebhooksPage = lazy(() => import("./pages/WebhooksPage"));
export const SourceControlPage = lazy(() => import("./pages/SourceControlPage"));
export { DevelopmentPanel } from "./components/DevelopmentPanel";
export { useWebhooks } from "./api";
