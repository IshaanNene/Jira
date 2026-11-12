import { GitBranch } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";

import { Badge, Button, Card, CardHeader, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Field, Input, Page, Select, Skeleton, toast } from "@/components/ui";
import { FormError } from "@/modules/auth";
import { can } from "@/modules/organizations";
import { errorMessage } from "@/services/api";
import { useSession } from "@/state/session";
import { formatRelative } from "@/utils/format";

import { useAssignWorkflow, useWorkflowAssignments, useWorkflowMutations, useWorkflows } from "../api";

const DEFAULT = "__default__";

export default function WorkflowsPage() {
  const role = useSession((s) => s.organization?.role);
  const admin = can(role, "manageOrganization");
  const workflows = useWorkflows();
  const { remove } = useWorkflowMutations();
  const [creating, setCreating] = useState(false);

  return (
    <Page
      title="Workflows"
      description="The statuses issues move through, and the rules for moving between them."
      actions={admin ? <Button variant="primary" onClick={() => setCreating(true)}>Create workflow</Button> : undefined}
    >
      <div className="space-y-5">
        {admin && <WorkspaceDefault />}
        <Card>
          {workflows.isPending ? (
            <div className="space-y-2 p-4" aria-busy><Skeleton className="h-10" /><Skeleton className="h-10" /></div>
          ) : workflows.isError ? (
            <ErrorState error={workflows.error} onRetry={() => void workflows.refetch()} />
          ) : workflows.data.length === 0 ? (
            <EmptyState
              icon={GitBranch}
              title="No custom workflows yet"
              description="Every project uses the built-in workflow (To Do → In Progress → In Review → Done, any order) until you create and assign your own."
              action={admin ? <Button variant="primary" onClick={() => setCreating(true)}>Create workflow</Button> : undefined}
            />
          ) : (
            <ul className="divide-y divide-line">
              {workflows.data.map((w) => (
                <li key={w.id} className="flex items-center gap-3 px-4 py-2.5">
                  <div className="min-w-0 flex-1">
                    <Link to={`/settings/workflows/${w.id}`} className="truncate text-base font-medium text-accent hover:underline">{w.name}</Link>
                    <div className="text-sm text-ink-muted">Updated {formatRelative(w.updated_at)}</div>
                  </div>
                  {w.published_version ? <Badge tone="success">Published v{w.published_version}</Badge> : <Badge>Never published</Badge>}
                  {w.has_draft && <Badge tone="warning">Draft</Badge>}
                  <Button asChild size="sm"><Link to={`/settings/workflows/${w.id}`}>{admin ? "Edit" : "View"}</Link></Button>
                  {admin && (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => {
                        if (!window.confirm(`Delete the workflow “${w.name}”?`)) return;
                        remove.mutate(w.id, { onSuccess: () => toast.success("Workflow deleted"), onError: (e) => toast.error("Couldn't delete workflow", errorMessage(e)) });
                      }}
                    >
                      Delete
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </Card>
      </div>
      <CreateDialog open={creating} onClose={() => setCreating(false)} />
    </Page>
  );
}

/** Which workflow applies to projects that have not chosen their own. */
function WorkspaceDefault() {
  const workflows = useWorkflows();
  const assignments = useWorkflowAssignments(null);
  const assign = useAssignWorkflow(null);
  const current = assignments.data?.find((a) => !a.issue_type)?.workflow_id ?? DEFAULT;
  const published = (workflows.data ?? []).filter((w) => w.published_version);
  return (
    <Card>
      <CardHeader title="Workspace default" description="Used by every project and issue type that doesn't have its own workflow." />
      <div className="p-4">
        <Select
          aria-label="Workspace default workflow"
          className="max-w-sm"
          value={current}
          disabled={assign.isPending || assignments.isPending}
          onValueChange={(v) =>
            assign.mutate({ issueType: null, workflowId: v === DEFAULT ? null : v }, { onSuccess: () => toast.success("Workspace default updated"), onError: (e) => toast.error("Couldn't change the default", errorMessage(e)) })
          }
          options={[{ value: DEFAULT, label: "Built-in workflow" }, ...published.map((w) => ({ value: w.id, label: w.name }))]}
        />
      </div>
    </Card>
  );
}

function CreateDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const navigate = useNavigate();
  const workflows = useWorkflows();
  const { create } = useWorkflowMutations();
  const [name, setName] = useState("");
  const [copyFrom, setCopyFrom] = useState(DEFAULT);
  const [error, setError] = useState<string | null>(null);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    create.mutate(
      { name, copy_from: copyFrom === DEFAULT ? undefined : copyFrom },
      {
        onSuccess: (workflow) => { setName(""); onClose(); navigate(`/settings/workflows/${workflow.id}`); },
        onError: (err) => setError(errorMessage(err)),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent title="Create workflow" description="Start from an existing workflow and change it in the designer.">
        <form onSubmit={submit} noValidate className="space-y-4">
          <FormError message={error} />
          <Field label="Name"><Input autoFocus maxLength={80} value={name} onChange={(e) => setName(e.target.value)} placeholder="Software delivery" /></Field>
          <Field label="Start from">
            <Select value={copyFrom} onValueChange={setCopyFrom} options={[{ value: DEFAULT, label: "Built-in workflow" }, ...(workflows.data ?? []).map((w) => ({ value: w.id, label: w.name }))]} />
          </Field>
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" loading={create.isPending} disabled={!name.trim()}>Create and open designer</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
