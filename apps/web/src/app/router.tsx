import { Suspense } from "react";
import { createBrowserRouter, Navigate, Outlet } from "react-router-dom";

import { BacklogPage, BoardPage, ReportsPage } from "@/modules/agile";
import { LoginPage, RegisterPage } from "@/modules/auth";
import { AcceptInvitationPage, OnboardingPage, OrganizationSettingsPage, PeoplePage } from "@/modules/organizations";
import { CustomFieldsPage, IssueListPage, IssuePage } from "@/modules/issues";
import { NotificationSettingsPage, UnsubscribePage } from "@/modules/notifications";
import { SearchPage } from "@/modules/search";
import { ProjectLive } from "@/modules/live";
import { ProjectLayout, ProjectSettingsPage, ProjectsPage } from "@/modules/projects";
import { ProfilePage, SessionsPage } from "@/modules/users";
import { WorkflowDesignerPage, WorkflowsPage } from "@/modules/workflows";
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
        ],
      },
      // Reachable signed in or out: shows a preview first, accepts once signed in.
      { path: "/invite/:token", element: <AcceptInvitationPage /> },
      // From notification emails; works signed out.
      { path: "/unsubscribe", element: <UnsubscribePage /> },
      {
        element: <RequireAuth />,
        children: [
          { path: "/onboarding", element: <OnboardingPage /> },
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
                      { path: "/settings/custom-fields", element: <CustomFieldsPage /> },
                      { path: "/settings/workflows", element: <WorkflowsPage /> },
                      { path: "/settings/profile", element: <ProfilePage /> },
                      { path: "/settings/notifications", element: <NotificationSettingsPage /> },
                      { path: "/settings/sessions", element: <SessionsPage /> },
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
