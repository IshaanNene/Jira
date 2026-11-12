import { Link } from "react-router-dom";

import { Card, CardHeader, ErrorState, Select, Skeleton, toast } from "@/components/ui";
import { errorMessage } from "@/services/api";

import { useAssignWorkflow, useIssueTypes, useWorkflowAssignments, useWorkflows } from "../api";

const INHERIT = "__inherit__";

/** Project settings tab: which workflow each issue type of this project follows. */
export function ProjectWorkflowSettings({ projectId }: { projectId: string }) {
  const workflows = useWorkflows();
  const assignments = useWorkflowAssignments(projectId);
  const types = useIssueTypes();
  const assign = useAssignWorkflow(projectId);
  const published = (workflows.data ?? []).filter((w) => w.published_version);
  const assigned = (issueType: string | null) => assignments.data?.find((a) => (a.issue_type ?? null) === issueType)?.workflow_id ?? INHERIT;
  const projectWide = assignments.data?.find((a) => !a.issue_type);

  const row = (label: string, issueType: string | null, inheritLabel: string) => (
    <li key={issueType ?? "all"} className="flex items-center gap-3 px-4 py-2">
      <span className="w-40 shrink-0 text-base text-ink">{label}</span>
      <div className="w-72 max-w-full">
        <Select
          size="sm"
          aria-label={`Workflow for ${label}`}
          value={assigned(issueType)}
          disabled={assign.isPending}
          onValueChange={(v) =>
            assign.mutate({ issueType, workflowId: v === INHERIT ? null : v }, { onSuccess: () => toast.success("Workflow updated"), onError: (e) => toast.error("Couldn't change the workflow", errorMessage(e)) })
          }
          options={[{ value: INHERIT, label: inheritLabel }, ...published.map((w) => ({ value: w.id, label: w.name }))]}
        />
      </div>
    </li>
  );

  return (
    <Card>
      <CardHeader
        title="Workflow"
        description="Choose the workflow issues in this project follow. An issue type without its own choice uses the project's; a project without one uses the workspace default."
      />
      {assignments.isPending || workflows.isPending ? (
        <div className="space-y-2 p-4" aria-busy><Skeleton className="h-9" /><Skeleton className="h-9" /></div>
      ) : assignments.isError ? (
        <ErrorState error={assignments.error} onRetry={() => void assignments.refetch()} />
      ) : (
        <>
          <ul className="divide-y divide-line">
            {row("All issue types", null, "Workspace default")}
            {(types.data ?? []).map((t) => row(t.name, t.key, projectWide ? `Same as project (${projectWide.workflow_name})` : "Same as project"))}
          </ul>
          <p className="border-t border-line px-4 py-3 text-sm text-ink-muted">
            Only published workflows can be chosen. <Link to="/settings/workflows" className="font-medium text-accent hover:underline">Manage workflows</Link>
          </p>
        </>
      )}
    </Card>
  );
}
