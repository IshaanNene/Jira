import { ClipboardCheck, FolderKanban } from "lucide-react";
import { Link } from "react-router-dom";

import { Button, EmptyState, ErrorState, Skeleton } from "@/components/ui";
import { IssueTypeIcon, PriorityIcon, StatusLozenge } from "@/modules/issues";
import { can } from "@/modules/organizations";
import { CreateProjectDialog, ProjectAvatar, PROJECT_TYPE_LABEL, useProjects } from "@/modules/projects";
import { statusLabel, useMqlSearch } from "@/modules/search";
import { useSession } from "@/state/session";

/**
 * Open work assigned to the signed-in person, across every project they can see, newest first.
 * "Open" is by status category, so it works whatever each workflow calls its statuses. The search
 * index trails edits by a few seconds.
 */
export const ASSIGNED_TO_ME = "assignee = currentUser() AND statusCategory != done ORDER BY updated DESC";

/** Landing page: your projects, and the open work assigned to you. */
export default function HomePage() {
  const role = useSession((s) => s.organization?.role);
  const projects = useProjects();
  const assigned = useMqlSearch(ASSIGNED_TO_ME);
  const firstPage = assigned.data?.pages[0];
  const issues = firstPage?.items ?? [];
  const canCreate = can(role, "viewMembers");

  return (
    <div className="mx-auto w-full max-w-6xl px-6 py-6">
      <h1 className="mb-5 text-2xl font-semibold tracking-tight text-ink">Your work</h1>

      <section className="mb-8">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-base font-semibold text-ink">Recent projects</h2>
          <Link to="/projects" className="text-sm font-medium text-accent hover:underline">View all projects</Link>
        </div>
        {projects.isPending ? (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4" aria-busy>{Array.from({ length: 4 }, (_, i) => <Skeleton key={i} className="h-24" />)}</div>
        ) : (projects.data ?? []).length === 0 ? (
          <EmptyState
            className="rounded-lg border border-dashed border-line"
            icon={FolderKanban}
            title="No projects yet"
            description={canCreate ? "Create a project to start tracking work." : "You'll see projects here once someone adds you to one."}
            action={canCreate ? <CreateProjectDialog trigger={<Button variant="primary">Create project</Button>} /> : undefined}
          />
        ) : (
          <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {(projects.data ?? []).slice(0, 8).map((p) => (
              <li key={p.id}>
                <Link to={`/projects/${p.key}/issues`} className="block rounded border border-line bg-surface p-3 shadow-xs transition-shadow hover:shadow-sm">
                  <div className="flex items-center gap-2.5">
                    <ProjectAvatar projectKey={p.key} size="lg" />
                    <div className="min-w-0">
                      <div className="truncate text-sm font-semibold text-ink">{p.name}</div>
                      <div className="truncate text-xs text-ink-subtle">{PROJECT_TYPE_LABEL[p.project_type]} · {p.key}</div>
                    </div>
                  </div>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section>
        <h2 className="mb-3 text-base font-semibold text-ink">Assigned to me</h2>
        {assigned.isPending ? (
          <div className="space-y-1.5" aria-busy><Skeleton className="h-9" /><Skeleton className="h-9" /><Skeleton className="h-9" /></div>
        ) : assigned.isError ? (
          <ErrorState className="rounded-lg border border-line" error={assigned.error} onRetry={() => void assigned.refetch()} />
        ) : issues.length === 0 ? (
          <EmptyState className="rounded-lg border border-dashed border-line" icon={ClipboardCheck} title="Nothing assigned to you" description="Open issues assigned to you will appear here." />
        ) : (
          <ul className="divide-y divide-line rounded border border-line">
            {issues.map((issue) => (
              <li key={issue.id}>
                <Link to={`/projects/${issue.project_key}/issues/${issue.key}`} className="flex items-center gap-3 px-3 py-2 hover:bg-sunken">
                  <IssueTypeIcon type={issue.type} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-base text-ink">{issue.summary}</span>
                    <span className="block text-xs text-ink-subtle">{issue.key}</span>
                  </span>
                  <PriorityIcon priority={issue.priority} />
                  <StatusLozenge name={statusLabel(issue.status)} category={issue.status_category} />
                </Link>
              </li>
            ))}
          </ul>
        )}
        {firstPage && firstPage.total > issues.length && (
          <Link to={`/search?mode=mql&mql=${encodeURIComponent(ASSIGNED_TO_ME)}`} className="mt-2 inline-block text-sm font-medium text-accent hover:underline">
            View all {firstPage.total_is_lower_bound ? `${firstPage.total}+` : firstPage.total} in search
          </Link>
        )}
      </section>
    </div>
  );
}
