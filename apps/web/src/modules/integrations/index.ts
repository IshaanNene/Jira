import { lazy } from "react";

export const WebhooksPage = lazy(() => import("./pages/WebhooksPage"));
export { useWebhooks } from "./api";
