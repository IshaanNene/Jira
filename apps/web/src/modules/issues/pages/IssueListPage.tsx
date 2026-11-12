import { ListTodo, Search } from "lucide-react";
import { useDeferredValue, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";

import { Button, EmptyState, ErrorState, Input, Select, Skeleton } from "@/components/ui";
import { UserChip, useUserDirectory } from "@/modules/organizations";
import { roleAtLeast, useProject } from "@/modules/projects";
import { useUi } from "@/state/ui";
import { formatRelative } from "@/utils/format";

import { useIssueMeta, useIssues, useProjectStatuses, type IssueFilters } from "../api";
import { IssueTypeIcon, PriorityIcon, StatusLozenge } from "../components/visuals";

const ANY = "any";

export default function IssueListPage() {
  const { projectKey = "" } = useParams();
  const key = projectKey.toUpperCase();
  const project = useProject(key);
  const meta = useIssueMeta();
  const { members } = useUserDirectory();
  const openCreate = useUi((s) => s.openCreateIssue);
  // Filters live in the URL so a filtered view can be bookmarked and shared.
  const [params, setParams] = useSearchParams();
  const [search, setSearch] = useState(params.get("q") ?? "");
  const q = useDeferredValue(search.trim());
  const filters: IssueFilters = {
    q: q || undefined,
    status: params.get("status") ?? undefined,
    type: params.get("type") ?? undefined,
    assignee: params.get("assignee") ?? undefined,
  };
  const issues = useIssues(key, filters);
  const rows = issues.data?.pages.flatMap((p) => p.items) ?? [];
  const statuses = useProjectStatuses(project.data?.id);
  const filtered = Boolean(filters.q || filters.status || filters.type || filters.assignee);
  const canCreate = roleAtLeast(project.data?.my_role, "member") && project.data?.status === "active";

  const setFilter = (name: string, value: string) => {
    const next = new URLSearchParams(params);
    if (value === ANY) next.delete(name);
    else next.set(name, value);
    setParams(next, { replace: true });
  };

  return (
    <div className="px-6 py-6">
      <nav aria-label="Breadcrumb" className="mb-1 text-sm text-ink-muted">
        <Link to="/projects" className="hover:underline">Projects</Link> / <span>{project.data?.name ?? key}</span>
      </nav>
      <header className="mb-4 flex items-center justify-between gap-3">
        <h1 className="text-2xl font-semibold tracking-tight text-ink">Issues</h1>
        {canCreate && <Button variant="primary" onClick={() => openCreate({ projectKey: key })}>Create issue</Button>}
      </header>

      <div className="mb-4 flex flex-wrap items-center gap-2">
        <div className="relative w-56 max-w-full">
          <Search className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-ink-subtle" aria-hidden />
          <Input type="search" aria-label="Search issues" placeholder="Search issues" value={search} onChange={(e) => setSearch(e.target.value)} className="pl-8" />
        </div>
        <div className="w-40"><Select aria-label="Status" value={filters.status ?? ANY} onValueChange={(v) => setFilter("status", v)} options={[{ value: ANY, label: "Status: All" }, ...(statuses.data ?? []).map((s) => ({ value: s.key, label: s.name }))]} /></div>
        <div className="w-40"><Select aria-label="Type" value={filters.type ?? ANY} onValueChange={(v) => setFilter("type", v)} options={[{ value: ANY, label: "Type: All" }, ...(meta.data?.types ?? []).map((t) => ({ value: t.key, label: t.name }))]} /></div>
        <div className="w-48"><Select aria-label="Assignee" value={filters.assignee ?? ANY} onValueChange={(v) => setFilter("assignee", v)} options={[{ value: ANY, label: "Assignee: All" }, { value: "me", label: "Assigned to me" }, { value: "unassigned", label: "Unassigned" }, ...members.map((m) => ({ value: m.user_id, label: m.display_name }))]} /></div>
        {filtered && <Button variant="ghost" onClick={() => { setSearch(""); setParams({}, { replace: true }); }}>Clear filters</Button>}
      </div>

      {issues.isPending ? (
        <div className="space-y-1.5" aria-busy aria-label="Loading issues">{Array.from({ length: 8 }, (_, i) => <Skeleton key={i} className="h-9" />)}</div>
      ) : issues.isError ? (
        <ErrorState error={issues.error} onRetry={() => void issues.refetch()} />
      ) : rows.length === 0 ? (
        <EmptyState
          className="rounded-lg border border-dashed border-line py-16"
          icon={ListTodo}
          title={filtered ? "No issues match your filters" : "No issues yet"}
          description={filtered ? "Try removing a filter or searching for something else." : "Issues track the work in this project: stories, tasks, bugs and more."}
          action={!filtered && canCreate ? <Button variant="primary" onClick={() => openCreate({ projectKey: key })}>Create issue</Button> : undefined}
        />
      ) : (
        <>
          <table className="w-full table-fixed text-left">
            <thead>
              <tr className="border-b-2 border-line text-xs font-semibold text-ink-muted">
                <th scope="col" className="w-8 px-2 py-2"><span className="sr-only">Type</span></th>
                <th scope="col" className="w-24 px-2 py-2">Key</th>
                <th scope="col" className="px-2 py-2">Summary</th>
                <th scope="col" className="w-32 px-2 py-2">Status</th>
                <th scope="col" className="hidden w-44 px-2 py-2 md:table-cell">Assignee</th>
                <th scope="col" className="w-10 px-2 py-2"><span className="sr-only">Priority</span></th>
                <th scope="col" className="hidden w-32 px-2 py-2 lg:table-cell">Updated</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((issue) => (
                <tr key={issue.id} className="border-b border-line hover:bg-sunken">
                  <td className="px-2 py-2"><IssueTypeIcon type={issue.type} /></td>
                  <td className="px-2 py-2"><Link to={`/projects/${key}/issues/${issue.key}`} className="whitespace-nowrap text-sm font-medium text-accent hover:underline">{issue.key}</Link></td>
                  <td className="truncate px-2 py-2">
                    <Link to={`/projects/${key}/issues/${issue.key}`} className={`text-base hover:underline ${issue.status_category === "done" ? "text-ink-muted line-through" : "text-ink"}`}>{issue.summary}</Link>
                    {issue.labels.map((l) => <span key={l} className="ml-2 rounded-sm bg-sunken px-1.5 py-0.5 text-xs text-ink-muted">{l}</span>)}
                  </td>
                  <td className="px-2 py-2"><StatusLozenge name={issue.status_name} category={issue.status_category} /></td>
                  <td className="hidden px-2 py-2 md:table-cell"><UserChip userId={issue.assignee_id} /></td>
                  <td className="px-2 py-2"><PriorityIcon priority={issue.priority} /></td>
                  <td className="hidden whitespace-nowrap px-2 py-2 text-sm text-ink-muted lg:table-cell">{formatRelative(issue.updated_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="flex items-center justify-between py-3 text-sm text-ink-muted">
            <span>{rows.length}{issues.hasNextPage ? "+" : ""} issue{rows.length === 1 ? "" : "s"}</span>
            {issues.hasNextPage && <Button size="sm" loading={issues.isFetchingNextPage} onClick={() => void issues.fetchNextPage()}>Load more</Button>}
          </div>
        </>
      )}
    </div>
  );
}
