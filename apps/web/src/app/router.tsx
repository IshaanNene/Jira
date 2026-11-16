import { Suspense } from "react";
import { createBrowserRouter, Navigate, Outlet } from "react-router-dom";

import { BacklogPage, BoardPage, ReportsPage } from "@/modules/agile";
import { DashboardPage, DashboardsPage } from "@/modules/dashboards";
import { ConfirmEmailChangePage, ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage, SsoCompletePage, SsoSignInPage, VerifyEmailPage } from "@/modules/auth";
import { AdminConsolePage, ProvisioningPage, SingleSignOnPage } from "@/modules/admin";
import { AcceptInvitationPage, AuditLogPage, OnboardingPage, OrganizationSettingsPage, PeoplePage } from "@/modules/organizations";
import { WebhooksPage } from "@/modules/integrations";
import { CustomFieldsPage, IssueListPage, IssuePage } from "@/modules/issues";
import { NotificationSettingsPage, UnsubscribePage } from "@/modules/notifications";
import { SearchPage } from "@/modules/search";
import { ProjectLive } from "@/modules/live";
import { ProjectLayout, ProjectSettingsPage, ProjectsPage } from "@/modules/projects";
import { ProfilePage, SecurityPage, TwoStepRequiredPage } from "@/modules/users";
import { WorkflowDesignerPage, WorkflowsPage } from "@/modules/workflows";
import { FeaturesPage, PricingPage, SecurityOverviewPage } from "@/modules/marketing";
import { HomePage } from "@/modules/workspace";

import { AppShell, SettingsLayout } from "./AppShell";
import { ErrorBoundary } from "./ErrorBoundary";
import { FullPageSpinner, RedirectIfAuthenticated, RequireAuth, RequireOrganization, SessionGate } from "./guards";
import { NotFoundPage } from "./NotFoundPage";

function Root() {
  return (
    <ErrorBoundary>
      <SessionGate>
        <Suspense fallback={<FullPageSpinner />}>
          <Outlet />
        </Suspense>
      </SessionGate>
    </ErrorBoundary>
  );
}

export const router = createBrowserRouter([
  {
    element: <Root />,
    children: [
      {
        element: <RedirectIfAuthenticated />,
        children: [
          { path: "/login", element: <LoginPage /> },
          { path: "/register", element: <RegisterPage /> },
          { path: "/forgot-password", element: <ForgotPasswordPage /> },
          { path: "/sso", element: <SsoSignInPage /> },
        ],
      },
      // Reachable signed in or out: shows a preview first, accepts once signed in.
      { path: "/invite/:token", element: <AcceptInvitationPage /> },
      // From account emails; work signed in or out.
      { path: "/verify-email", element: <VerifyEmailPage /> },
      { path: "/reset-password", element: <ResetPasswordPage /> },
      // Back from a workspace's identity provider, with the session cookie just set.
      { path: "/sso/complete", element: <SsoCompletePage /> },
      { path: "/confirm-email", element: <ConfirmEmailChangePage /> },
      // The public site; works signed in or out.
      { path: "/features", element: <FeaturesPage /> },
      { path: "/pricing", element: <PricingPage /> },
      { path: "/security", element: <SecurityOverviewPage /> },
      // From notification emails; works signed out.
      { path: "/unsubscribe", element: <UnsubscribePage /> },
      {
        element: <RequireAuth />,
        children: [
          { path: "/onboarding", element: <OnboardingPage /> },
          { path: "/two-step-required", element: <TwoStepRequiredPage /> },
          {
            element: <RequireOrganization />,
            children: [
              {
                element: <AppShell />,
                children: [
                  { path: "/", element: <HomePage /> },
                  { path: "/people", element: <PeoplePage /> },
                  { path: "/search", element: <SearchPage /> },
                  { path: "/projects", element: <ProjectsPage /> },
                  { path: "/dashboards", element: <DashboardsPage /> },
                  { path: "/dashboards/:dashboardId", element: <DashboardPage /> },
                  {
                    path: "/projects/:projectKey",
                    element: (
                      <>
                        <ProjectLive />
                        <ProjectLayout />
                      </>
                    ),
                    children: [
                      { index: true, element: <Navigate to="board" replace /> },
                      { path: "board", element: <BoardPage /> },
                      { path: "backlog", element: <BacklogPage /> },
                      { path: "reports", element: <ReportsPage /> },
                      { path: "issues", element: <IssueListPage /> },
                      { path: "issues/:issueKey", element: <IssuePage /> },
                      { path: "settings/*", element: <ProjectSettingsPage /> },
                    ],
                  },
                  // Full-width: the designer needs the whole canvas.
                  { path: "/settings/workflows/:workflowId", element: <WorkflowDesignerPage /> },
                  {
                    element: <SettingsLayout />,
                    children: [
                      { path: "/settings/workspace", element: <OrganizationSettingsPage /> },
                      { path: "/settings/admin", element: <AdminConsolePage /> },
                      { path: "/settings/sso", element: <SingleSignOnPage /> },
                      { path: "/settings/provisioning", element: <ProvisioningPage /> },
                      { path: "/settings/custom-fields", element: <CustomFieldsPage /> },
                      { path: "/settings/audit-log", element: <AuditLogPage /> },
                      { path: "/settings/webhooks", element: <WebhooksPage /> },
                      { path: "/settings/workflows", element: <WorkflowsPage /> },
                      { path: "/settings/profile", element: <ProfilePage /> },
                      { path: "/settings/notifications", element: <NotificationSettingsPage /> },
                      { path: "/settings/security", element: <SecurityPage /> },
                      { path: "/settings/sessions", element: <Navigate to="/settings/security" replace /> },
                    ],
                  },
                  { path: "*", element: <NotFoundPage /> },
                ],
              },
            ],
          },
        ],
      },
    ],
  },
]);
