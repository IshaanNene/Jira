import { lazy } from "react";

export { orgKeys, useCurrentOrganization, useInvitations, useMembers, useMyOrganizations, useUserDirectory } from "./api";
export { UserChip } from "./components/UserChip";
export { RoleBadge } from "./components/RoleBadge";
export { can, ROLE_LABEL, type OrgPermission } from "./permissions";

// Route components are code-split; other modules reach them only through this index.
export const OnboardingPage = lazy(() => import("./pages/OnboardingPage"));
export const PeoplePage = lazy(() => import("./pages/PeoplePage"));
export const AuditLogPage = lazy(() => import("./pages/AuditLogPage"));
export const AcceptInvitationPage = lazy(() => import("./pages/AcceptInvitationPage"));
export const OrganizationSettingsPage = lazy(() => import("./pages/OrganizationSettingsPage"));
