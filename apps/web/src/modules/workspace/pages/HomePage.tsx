import { useQueries } from "@tanstack/react-query";
import { ClipboardCheck, FolderKanban } from "lucide-react";
import { Link } from "react-router-dom";

import { Button, EmptyState, Skeleton } from "@/components/ui";
import { issueKeys, IssueTypeIcon, PriorityIcon, StatusLozenge } from "@/modules/issues";
import { can } from "@/modules/organizations";
import { CreateProjectDialog, ProjectAvatar, PROJECT_TYPE_LABEL, useProjects } from "@/modules/projects";
import { api, type IssuePage } from "@/services/api";
import { useSession } from "@/state/session";

/** How many projects "Assigned to me" looks through until cross-project search exists (Phase 6). */
const MAX_PROJECTS = 8;

/** Landing page: your projects, and the open work assigned to you. */
export default function HomePage() {
  const orgId = useSession((s) => s.organization?.id);
  const role = useSession((s) => s.organization?.role);
  const projects = useProjects();
  const scanned = (projects.data ?? []).slice(0, MAX_PROJECTS);
  // "Open" is defined by status category, so it works whatever each project's workflow calls its statuses.
  const filters = { assignee: "me", status_category: "todo,in_progress" };
  const assigned = useQueries({
    queries: scanned.map((p) => ({
      queryKey: issueKeys.list(orgId, p.key, filters),
      queryFn: () => api.get<IssuePage>("/api/v1/issues", { query: { project: p.key, limit: 20, ...filters } }),
      select: (d: IssuePage) => d.items,
    })),
  });
  const issues = assigned.flatMap((q) => q.data ?? []).sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  const loadingIssues = projects.isPending || assigned.some((q) => q.isPending);
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
        {loadingIssues ? (
          <div className="space-y-1.5" aria-busy><Skeleton className="h-9" /><Skeleton className="h-9" /><Skeleton className="h-9" /></div>
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
                  <StatusLozenge name={issue.status_name} category={issue.status_category} />
                </Link>
              </li>
            ))}
          </ul>
        )}
        {(projects.data?.length ?? 0) > MAX_PROJECTS && (
          <p className="mt-2 text-xs text-ink-subtle">Showing work from your first {MAX_PROJECTS} projects.</p>
        )}
      </section>
    </div>
  );
}
