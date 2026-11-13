import { lazy } from "react";

export { useMyProfile } from "./api";

export const ProfilePage = lazy(() => import("./pages/ProfilePage"));
export const SecurityPage = lazy(() => import("./pages/SecurityPage"));
