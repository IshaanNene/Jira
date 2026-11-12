import { lazy } from "react";

export { useMyProfile } from "./api";

export const ProfilePage = lazy(() => import("./pages/ProfilePage"));
export const SessionsPage = lazy(() => import("./pages/SessionsPage"));
