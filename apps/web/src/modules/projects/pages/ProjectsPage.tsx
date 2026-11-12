import { FolderKanban, Lock, Search } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router-dom";

import { Badge, Button, EmptyState, ErrorState, Input, Page, Skeleton } from "@/components/ui";
import { can, UserChip } from "@/modules/organizations";
import { useSession } from "@/state/session";

import { useProjects } from "../api";
import { CreateProjectDialog } from "../components/CreateProjectDialog";
import { ProjectAvatar } from "../components/ProjectAvatar";
import { PROJECT_TYPE_LABEL } from "../roles";

export default function ProjectsPage() {
  const role = useSession((s) => s.organization?.role);
  const [status, setStatus] = useState<"active" | "archived">("active");
  const [search, setSearch] = useState("");
  const projects = useProjects(status);
  // Guests are added to projects; they do not create them (the server enforces this).
  const canCreate = can(role, "viewMembers");
  const term = search.trim().toLowerCase();
  const rows = (projects.data ?? []).filter((p) => !term || p.name.toLowerCase().includes(term) || p.key.toLowerCase().includes(term));

  return (
    <Page
      title="Projects"
      width="wide"
      actions={canCreate ? <CreateProjectDialog trigger={<Button variant="primary">Create project</Button>} /> : undefined}
    >
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <div className="relative w-64 max-w-full">
          <Search className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-ink-subtle" aria-hidden />
          <Input type="search" aria-label="Search projects" placeholder="Search projects" value={search} onChange={(e) => setSearch(e.target.value)} className="pl-8" />
        </div>
        <div className="flex rounded border border-line-strong p-0.5" role="group" aria-label="Project status">
          {(["active", "archived"] as const).map((s) => (
            <button
              key={s}
              type="button"
              aria-pressed={status === s}
              onClick={() => setStatus(s)}
              className={`h-7 rounded-sm px-2.5 text-sm font-medium capitalize ${status === s ? "bg-accent-soft text-accent" : "text-ink-muted hover:text-ink"}`}
            >
              {s}
            </button>
          ))}
        </div>
      </div>

      {projects.isPending ? (
        <div className="space-y-2" aria-busy>{Array.from({ length: 4 }, (_, i) => <Skeleton key={i} className="h-11" />)}</div>
      ) : projects.isError ? (
        <ErrorState error={projects.error} onRetry={() => void projects.refetch()} />
      ) : rows.length === 0 ? (
        <EmptyState
          className="rounded-lg border border-dashed border-line py-16"
          icon={FolderKanban}
          title={term ? "No projects match" : status === "archived" ? "No archived projects" : "No projects yet"}
          description={
            term ? "Try a different name or key." : status === "archived" ? "Archived projects are read-only and appear here." : canCreate ? "Create your first project to start tracking issues." : "You'll see projects here once someone adds you to one."
          }
          action={!term && status === "active" && canCreate ? <CreateProjectDialog trigger={<Button variant="primary">Create project</Button>} /> : undefined}
        />
      ) : (
        <table className="w-full text-left">
          <thead>
            <tr className="border-b-2 border-line text-xs font-semibold text-ink-muted">
              <th scope="col" className="px-2 py-2">Name</th>
              <th scope="col" className="w-28 px-2 py-2">Key</th>
              <th scope="col" className="hidden w-32 px-2 py-2 sm:table-cell">Type</th>
              <th scope="col" className="hidden w-56 px-2 py-2 md:table-cell">Lead</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((project) => (
              <tr key={project.id} className="border-b border-line hover:bg-sunken">
                <td className="px-2 py-2">
                  <Link to={`/projects/${project.key}/issues`} className="flex items-center gap-2.5 font-medium text-accent hover:underline">
                    <ProjectAvatar projectKey={project.key} />
                    <span className="truncate">{project.name}</span>
                    {!project.default_role && <Lock className="h-3 w-3 shrink-0 text-ink-subtle" aria-label="Private" />}
                    {project.status === "archived" && <Badge>Archived</Badge>}
                  </Link>
                </td>
                <td className="px-2 py-2 font-mono text-sm text-ink-muted">{project.key}</td>
                <td className="hidden px-2 py-2 text-sm text-ink-muted sm:table-cell">{PROJECT_TYPE_LABEL[project.project_type] ?? project.project_type}</td>
                <td className="hidden px-2 py-2 md:table-cell"><UserChip userId={project.lead_user_id} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Page>
  );
}
