import { ArrowLeft, Lock, Plus } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { Link, useParams } from "react-router-dom";

import { Badge, Button, EmptyState, ErrorState, Input, Skeleton, toast } from "@/components/ui";
import { can } from "@/modules/organizations";
import { ApiError, errorMessage, type WorkflowDefinition, type WorkflowDetail } from "@/services/api";
import { useSession } from "@/state/session";

import { useWorkflow, useWorkflowMutations } from "../api";
import { Canvas, type Selection } from "../components/Canvas";
import { Inspector } from "../components/Inspector";
import { addStatus, addTransition, moveStatus } from "../model";

const EMPTY: WorkflowDefinition = { statuses: [], initial: "", transitions: [] };

export default function WorkflowDesignerPage() {
  const { workflowId = "" } = useParams();
  const workflow = useWorkflow(workflowId);
  if (workflow.isPending) return <div className="space-y-3 p-6" aria-busy><Skeleton className="h-8 w-64" /><Skeleton className="h-96" /></div>;
  if (workflow.isError) {
    return workflow.error instanceof ApiError && workflow.error.status === 404 ? (
      <EmptyState className="py-24" icon={Lock} title="Workflow not found" description="It may have been deleted." action={<Button asChild variant="primary"><Link to="/settings/workflows">All workflows</Link></Button>} />
    ) : (
      <ErrorState className="py-24" error={workflow.error} onRetry={() => void workflow.refetch()} />
    );
  }
  // Keyed by id so switching workflows starts from a clean working copy.
  return <Designer key={workflow.data.id} detail={workflow.data} />;
}

function Designer({ detail }: { detail: WorkflowDetail }) {
  const role = useSession((s) => s.organization?.role);
  const readOnly = !can(role, "manageOrganization");
  const { saveDraft, publish, discardDraft } = useWorkflowMutations();
  const serverDefinition = detail.draft ?? detail.published ?? EMPTY;
  // The working copy lives here; nothing reaches the server until Save or Publish.
  const [definition, setDefinition] = useState<WorkflowDefinition>(serverDefinition);
  const [dirty, setDirty] = useState(false);
  const [selection, setSelection] = useState<Selection>(null);
  const [connectingFrom, setConnectingFrom] = useState<string | null>(null);
  const [newStatus, setNewStatus] = useState("");
  // The version edits are based on: sent as If-Match so two editors cannot overwrite each other.
  const version = useRef(detail.version);
  version.current = detail.version;

  const change = (next: WorkflowDefinition) => {
    setDefinition(next);
    setDirty(true);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setConnectingFrom(null);
    const warn = (e: BeforeUnloadEvent) => dirty && e.preventDefault();
    window.addEventListener("keydown", onKey);
    window.addEventListener("beforeunload", warn);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("beforeunload", warn);
    };
  }, [dirty]);

  const conflict = (error: unknown, title: string) => {
    if (error instanceof ApiError && error.isConflict) {
      toast.error("Someone else changed this workflow", "Reload the page to see their version before continuing.");
    } else {
      toast.error(title, error instanceof ApiError && error.fieldErrors[0] ? error.fieldErrors[0].message : errorMessage(error));
    }
  };

  const save = async (): Promise<WorkflowDetail | null> => {
    try {
      const saved = await saveDraft.mutateAsync({ id: detail.id, version: version.current, definition });
      setDirty(false);
      return saved;
    } catch (error) {
      conflict(error, "Couldn't save the draft");
      return null;
    }
  };

  const onSave = async () => {
    const saved = await save();
    if (saved) toast.success("Draft saved", saved.problems.length ? "It has problems to fix before it can be published." : undefined);
  };

  const onPublish = async () => {
    // Publish what is on screen: save first if there are unsaved edits.
    const base = dirty ? await save() : detail;
    if (!base) return;
    if (base.problems.length > 0) {
      toast.error("This workflow can't be published yet", base.problems[0]?.message);
      setSelection(null);
      return;
    }
    try {
      const published = await publish.mutateAsync({ id: detail.id, version: base.version });
      toast.success(`Published version ${published.published_version}`, "Issues using this workflow follow the new rules from now on.");
    } catch (error) {
      conflict(error, "Couldn't publish");
    }
  };

  const onDiscard = async () => {
    if (!window.confirm("Discard all unpublished changes to this workflow?")) return;
    try {
      const restored = await discardDraft.mutateAsync({ id: detail.id, version: version.current });
      setDefinition(restored.published ?? EMPTY);
      setDirty(false);
      setSelection(null);
    } catch (error) {
      conflict(error, "Couldn't discard the draft");
    }
  };

  const onAddStatus = (e: FormEvent) => {
    e.preventDefault();
    if (!newStatus.trim()) return;
    const added = addStatus(definition, newStatus);
    change(added.definition);
    setSelection({ kind: "status", key: added.key });
    setNewStatus("");
  };

  const hasDraft = detail.has_draft || dirty;
  const problems = dirty ? [] : detail.problems;
  const busy = saveDraft.isPending || publish.isPending || discardDraft.isPending;

  return (
    <div className="flex h-full flex-col">
      <header className="flex flex-wrap items-center gap-2 border-b border-line bg-surface px-4 py-2.5">
        <Button asChild variant="ghost" size="icon"><Link to="/settings/workflows" aria-label="Back to workflows"><ArrowLeft className="h-4 w-4" /></Link></Button>
        <h1 className="mr-1 truncate text-lg font-semibold text-ink">{detail.name}</h1>
        {detail.published_version ? <Badge tone="success">Published v{detail.published_version}</Badge> : <Badge>Never published</Badge>}
        {hasDraft && <Badge tone="warning">{dirty ? "Unsaved changes" : "Unpublished draft"}</Badge>}
        {readOnly ? (
          <span className="ml-auto text-sm text-ink-subtle">View only — workspace admins edit workflows.</span>
        ) : (
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <form onSubmit={onAddStatus} className="flex items-center gap-1">
              <Input aria-label="New status name" placeholder="New status name" value={newStatus} maxLength={40} onChange={(e) => setNewStatus(e.target.value)} className="h-8 w-40" />
              <Button type="submit" disabled={!newStatus.trim()}><Plus className="h-3.5 w-3.5" aria-hidden /> Add status</Button>
            </form>
            {detail.has_draft && detail.published_version !== null && <Button variant="ghost" disabled={busy} onClick={() => void onDiscard()}>Discard draft</Button>}
            <Button disabled={!dirty || busy} loading={saveDraft.isPending} onClick={() => void onSave()}>Save draft</Button>
            <Button variant="primary" disabled={!hasDraft || busy} loading={publish.isPending} onClick={() => void onPublish()}>Publish</Button>
          </div>
        )}
      </header>
      {connectingFrom && (
        <div role="status" className="flex items-center justify-between bg-warning-soft px-4 py-1.5 text-sm text-warning">
          <span>Click the status this transition should lead to.</span>
          <button type="button" className="font-medium underline" onClick={() => setConnectingFrom(null)}>Cancel (Esc)</button>
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1">
          {definition.statuses.length === 0 ? (
            <EmptyState className="py-24" icon={Plus} title="No statuses yet" description="Add a status above to start drawing this workflow." />
          ) : (
            <Canvas
              definition={definition}
              selection={selection}
              onSelect={setSelection}
              connectingFrom={connectingFrom}
              onStartConnect={setConnectingFrom}
              onConnect={(from, to) => {
                const added = addTransition(definition, from, to);
                change(added.definition);
                setConnectingFrom(null);
                setSelection({ kind: "transition", id: added.id });
              }}
              onMove={(key, dx, dy) => change(moveStatus(definition, key, dx, dy))}
              readOnly={readOnly}
            />
          )}
        </div>
        <aside className="w-80 shrink-0 overflow-y-auto border-l border-line bg-surface p-4" aria-label="Properties">
          <Inspector definition={definition} selection={selection} problems={problems} readOnly={readOnly} onChange={change} onSelect={setSelection} />
        </aside>
      </div>
    </div>
  );
}
