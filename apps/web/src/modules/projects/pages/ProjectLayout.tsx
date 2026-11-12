import { Archive, BarChart3, KanbanSquare, ListTodo, Rows3, Settings } from "lucide-react";
import { Suspense } from "react";
import { Link, NavLink, Outlet, useParams } from "react-router-dom";

import { Button, EmptyState, ErrorState, Skeleton } from "@/components/ui";
import { ApiError } from "@/services/api";
import { cn } from "@/utils/cn";

import { useProject } from "../api";
import { ProjectAvatar } from "../components/ProjectAvatar";
import { PROJECT_TYPE_LABEL, roleAtLeast } from "../roles";

/** Project frame: contextual sidebar on the left, the project's pages on the right. */
export default function ProjectLayout() {
  const { projectKey = "" } = useParams();
  const key = projectKey.toUpperCase();
  const project = useProject(key);

  if (project.isError) {
    const missing = project.error instanceof ApiError && project.error.status === 404;
    return missing ? (
      <EmptyState
        className="py-24"
        icon={Archive}
        title="Project not found"
        description="It may have been deleted, or you may not have access to it."
        action={<Button asChild variant="primary"><Link to="/projects">View all projects</Link></Button>}
      />
    ) : (
      <ErrorState className="py-24" error={project.error} onRetry={() => void project.refetch()} />
    );
  }

  const link = ({ isActive }: { isActive: boolean }) =>
    cn("flex h-9 items-center gap-3 rounded px-3 text-sm font-medium", isActive ? "bg-accent-soft text-accent" : "text-ink-muted hover:bg-sunken hover:text-ink");

  return (
    <div className="flex h-full">
      <aside className="hidden w-60 shrink-0 flex-col border-r border-line bg-sunken/60 md:flex" aria-label="Project">
        <div className="flex items-center gap-3 px-4 py-5">
          <ProjectAvatar projectKey={key} size="lg" />
          <div className="min-w-0">
            {project.data ? (
              <>
                <div className="truncate text-sm font-semibold text-ink">{project.data.name}</div>
                <div className="truncate text-xs text-ink-subtle">{PROJECT_TYPE_LABEL[project.data.project_type]} project</div>
              </>
            ) : (
              <Skeleton className="h-8 w-32" />
            )}
          </div>
        </div>
        <nav className="space-y-0.5 px-2">
          <NavLink to={`/projects/${key}/board`} className={link}><KanbanSquare className="h-4 w-4" aria-hidden /> Board</NavLink>
          <NavLink to={`/projects/${key}/backlog`} className={link}><Rows3 className="h-4 w-4" aria-hidden /> Backlog</NavLink>
          <NavLink to={`/projects/${key}/reports`} className={link}><BarChart3 className="h-4 w-4" aria-hidden /> Reports</NavLink>
          <NavLink to={`/projects/${key}/issues`} className={link}><ListTodo className="h-4 w-4" aria-hidden /> Issues</NavLink>
          {roleAtLeast(project.data?.my_role, "administrator") && (
            <NavLink to={`/projects/${key}/settings`} className={link}><Settings className="h-4 w-4" aria-hidden /> Project settings</NavLink>
          )}
        </nav>
        {project.data?.status === "archived" && (
          <p className="mx-3 mt-4 rounded border border-warning/30 bg-warning-soft px-3 py-2 text-xs text-warning">This project is archived and read-only.</p>
        )}
      </aside>
      <div className="min-w-0 flex-1 overflow-y-auto">
        <Suspense fallback={<div className="space-y-3 p-6" aria-busy><Skeleton className="h-7 w-48" /><Skeleton className="h-40" /></div>}>
          <Outlet />
        </Suspense>
      </div>
    </div>
  );
}
