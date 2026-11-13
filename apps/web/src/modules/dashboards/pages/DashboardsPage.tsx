import { LayoutDashboard, Search, Users } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router-dom";

import { Badge, Button, EmptyState, ErrorState, Input, Page, Skeleton } from "@/components/ui";
import { UserChip } from "@/modules/organizations";
import { useSession } from "@/state/session";
import { formatRelative } from "@/utils/format";

import { useDashboards } from "../api";
import { DashboardDialog } from "../components/DashboardDialog";

export default function DashboardsPage() {
  const me = useSession((s) => s.account?.id);
  const dashboards = useDashboards();
  const [search, setSearch] = useState("");
  const term = search.trim().toLowerCase();
  const rows = (dashboards.data ?? []).filter((d) => !term || d.name.toLowerCase().includes(term));

  return (
    <Page title="Dashboards" width="wide" actions={<DashboardDialog trigger={<Button variant="primary">Create dashboard</Button>} />}>
      <div className="mb-4 relative w-64 max-w-full">
        <Search className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-ink-subtle" aria-hidden />
        <Input type="search" aria-label="Search dashboards" placeholder="Search dashboards" value={search} onChange={(e) => setSearch(e.target.value)} className="pl-8" />
      </div>

      {dashboards.isPending ? (
        <div className="space-y-2" aria-busy>{Array.from({ length: 3 }, (_, i) => <Skeleton key={i} className="h-11" />)}</div>
      ) : dashboards.isError ? (
        <ErrorState error={dashboards.error} onRetry={() => void dashboards.refetch()} />
      ) : rows.length === 0 ? (
        <EmptyState
          className="rounded-lg border border-dashed border-line py-16"
          icon={LayoutDashboard}
          title={term ? "No dashboards match" : "No dashboards yet"}
          description={term ? "Try a different name." : "Create a dashboard to chart how work moves across your projects."}
        />
      ) : (
        <div className="overflow-x-auto rounded-lg border border-line">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-line bg-sunken text-left text-xs uppercase tracking-wide text-ink-subtle">
                <th scope="col" className="px-4 py-2 font-semibold">Name</th>
                <th scope="col" className="px-4 py-2 font-semibold">Owner</th>
                <th scope="col" className="px-4 py-2 text-right font-semibold">Charts</th>
                <th scope="col" className="px-4 py-2 font-semibold">Updated</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((d) => (
                <tr key={d.id} className="border-b border-line bg-surface last:border-0 hover:bg-sunken">
                  <td className="px-4 py-2">
                    <span className="flex items-center gap-2">
                      <LayoutDashboard className="h-4 w-4 shrink-0 text-ink-subtle" aria-hidden />
                      <Link to={`/dashboards/${d.id}`} className="font-medium text-accent hover:underline">{d.name}</Link>
                      {d.shared && <Badge><Users className="mr-1 h-3 w-3" aria-hidden />Shared</Badge>}
                    </span>
                    {d.description && <p className="ml-6 mt-0.5 line-clamp-1 text-xs text-ink-muted">{d.description}</p>}
                  </td>
                  <td className="px-4 py-2">{d.owner_id === me ? <span className="text-ink-muted">You</span> : <UserChip userId={d.owner_id} />}</td>
                  <td className="px-4 py-2 text-right tabular-nums text-ink-muted">{d.widget_count}</td>
                  <td className="px-4 py-2 text-ink-muted">{formatRelative(d.updated_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Page>
  );
}
