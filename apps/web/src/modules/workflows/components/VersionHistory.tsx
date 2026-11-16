import { ArrowRight, History } from "lucide-react";
import { useState } from "react";

import { Badge, Button, Dialog, DialogContent, DialogFooter, ErrorState, Skeleton } from "@/components/ui";
import { useUserDirectory } from "@/modules/organizations";
import type { WorkflowDefinition } from "@/services/api";
import { cn } from "@/utils/cn";
import { formatRelative } from "@/utils/format";

import { useWorkflowVersion, useWorkflowVersions } from "../api";

interface Props {
  workflowId: string;
  publishedVersion: number | null;
  canRestore: boolean;
  restoring: boolean;
  /** Resolves true when the version became the draft. */
  onRestore: (version: number) => Promise<boolean>;
}

/** Published versions of a workflow; an admin can make one the draft again. */
export function VersionHistory({ workflowId, publishedVersion, canRestore, restoring, onRestore }: Props) {
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  const versions = useWorkflowVersions(workflowId, open);
  const chosen = useWorkflowVersion(workflowId, open ? selected : null);
  const { byId } = useUserDirectory();

  const close = (next: boolean) => {
    setOpen(next);
    if (!next) setSelected(null);
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <Button variant="ghost" disabled={publishedVersion === null} onClick={() => setOpen(true)} title={publishedVersion === null ? "Nothing has been published yet" : undefined}>
        <History className="h-3.5 w-3.5" aria-hidden /> Versions
      </Button>
      {open && (
        <DialogContent title="Version history" description="Each publish is kept as it was. Restoring one makes it the draft; issues follow it only once you publish." className="max-w-3xl">
          {versions.isPending ? (
            <div className="space-y-2" aria-busy><Skeleton className="h-10" /><Skeleton className="h-10" /></div>
          ) : versions.isError ? (
            <ErrorState error={versions.error} onRetry={() => void versions.refetch()} />
          ) : (
            <div className="grid gap-4 sm:grid-cols-[14rem_minmax(0,1fr)]">
              <ul className="max-h-80 space-y-1 overflow-y-auto" aria-label="Versions">
                {versions.data.map((v) => (
                  <li key={v.version}>
                    <button
                      type="button"
                      aria-pressed={selected === v.version}
                      onClick={() => setSelected(v.version)}
                      className={cn("w-full rounded px-2.5 py-2 text-left hover:bg-sunken", selected === v.version && "bg-accent-soft")}
                    >
                      <span className="flex items-center gap-2 text-base font-medium text-ink">
                        Version {v.version} {v.version === publishedVersion && <Badge tone="success">Published</Badge>}
                      </span>
                      <span className="block text-sm text-ink-muted">
                        {formatRelative(v.published_at)}
                        {byId.get(v.published_by) ? ` by ${byId.get(v.published_by)!.display_name}` : ""}
                      </span>
                      <span className="block text-xs text-ink-subtle">{v.status_count} statuses · {v.transition_count} transitions</span>
                    </button>
                  </li>
                ))}
              </ul>
              <div className="min-h-40 rounded border border-line p-3" aria-live="polite">
                {selected === null ? (
                  <p className="text-sm text-ink-muted">Choose a version to see its statuses and transitions.</p>
                ) : chosen.isPending ? (
                  <div className="space-y-2" aria-busy><Skeleton className="h-5" /><Skeleton className="h-5" /></div>
                ) : chosen.isError ? (
                  <ErrorState error={chosen.error} onRetry={() => void chosen.refetch()} />
                ) : (
                  <Outline definition={chosen.data.definition} />
                )}
              </div>
            </div>
          )}
          <DialogFooter>
            <Button onClick={() => close(false)}>Close</Button>
            {canRestore && (
              <Button
                variant="primary"
                disabled={selected === null || selected === publishedVersion}
                loading={restoring}
                onClick={() => {
                  if (selected === null) return;
                  void onRestore(selected).then((done) => done && close(false));
                }}
              >
                {selected === null ? "Restore as draft" : `Restore version ${selected} as draft`}
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      )}
    </Dialog>
  );
}

function Outline({ definition }: { definition: WorkflowDefinition }) {
  const name = (key: string) => definition.statuses.find((s) => s.key === key)?.name ?? key;
  return (
    <div className="space-y-3 text-sm">
      <div>
        <h3 className="mb-1 font-semibold text-ink">Statuses</h3>
        <ul className="flex flex-wrap gap-1.5">
          {definition.statuses.map((s) => (
            <li key={s.key}><Badge>{s.name}{s.key === definition.initial ? " (first)" : ""}</Badge></li>
          ))}
        </ul>
      </div>
      <div>
        <h3 className="mb-1 font-semibold text-ink">Transitions</h3>
        <ul className="space-y-1 text-ink-muted">
          {definition.transitions.map((t) => (
            <li key={t.id} className="flex flex-wrap items-center gap-1">
              <span className="font-medium text-ink">{t.name}</span>:
              <span>{t.from?.length ? t.from.map(name).join(", ") : "Any status"}</span>
              <ArrowRight className="h-3 w-3" aria-hidden />
              <span>{name(t.to)}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
