import { Plus, X } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";

import { Button, Input, Select, Skeleton, toast } from "@/components/ui";
import { UserChip } from "@/modules/organizations";
import { errorMessage, type IssueDetail } from "@/services/api";
import { useUi } from "@/state/ui";

import { useIssueMeta, useIssues, useLinkMutations, useLinks } from "../api";
import { IssueTypeIcon, PriorityIcon, StatusLozenge } from "./visuals";

function SectionTitle({ title, action }: { title: string; action?: React.ReactNode }) {
  return (
    <div className="mb-2 flex items-center justify-between">
      <h2 className="text-base font-semibold text-ink">{title}</h2>
      {action}
    </div>
  );
}

export function ChildIssues({ issue, canEdit }: { issue: IssueDetail; canEdit: boolean }) {
  const meta = useIssueMeta();
  const openCreate = useUi((s) => s.openCreateIssue);
  const children = useIssues(issue.project_key, { parent: issue.key });
  const rows = children.data?.pages.flatMap((p) => p.items) ?? [];
  const level = meta.data?.types.find((t) => t.key === issue.type)?.level ?? 0;
  if (level < 0) return null; // subtasks have no children
  if (rows.length === 0 && !canEdit) return null;
  const done = rows.filter((r) => r.status_category === "done").length;

  return (
    <section>
      <SectionTitle
        title="Child issues"
        action={canEdit && (
          <Button size="sm" variant="ghost" onClick={() => openCreate({ projectKey: issue.project_key, parentKey: issue.key })}>
            <Plus className="h-3.5 w-3.5" aria-hidden /> Add child issue
          </Button>
        )}
      />
      {children.isPending ? (
        <Skeleton className="h-9" />
      ) : rows.length === 0 ? (
        <p className="text-sm text-ink-subtle">No child issues.</p>
      ) : (
        <>
          <div className="mb-2 flex items-center gap-2" aria-label={`${done} of ${rows.length} done`}>
            <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-line">
              <div className="h-full bg-success transition-[width]" style={{ width: `${(done / rows.length) * 100}%` }} />
            </div>
            <span className="tabular text-xs text-ink-muted">{Math.round((done / rows.length) * 100)}% done</span>
          </div>
          <ul className="divide-y divide-line rounded border border-line">
            {rows.map((child) => (
              <li key={child.id} className="flex items-center gap-2.5 px-2.5 py-1.5 hover:bg-sunken">
                <IssueTypeIcon type={child.type} />
                <Link to={`/projects/${child.project_key}/issues/${child.key}`} className="shrink-0 text-sm font-medium text-accent hover:underline">{child.key}</Link>
                <span className="min-w-0 flex-1 truncate text-sm text-ink">{child.summary}</span>
                <PriorityIcon priority={child.priority} />
                <UserChip userId={child.assignee_id} showName={false} />
                <StatusLozenge name={child.status_name} category={child.status_category} />
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}

export function LinkedIssues({ issue, canEdit }: { issue: IssueDetail; canEdit: boolean }) {
  const meta = useIssueMeta();
  const links = useLinks(issue.key);
  const { add, remove } = useLinkMutations(issue.key);
  const [adding, setAdding] = useState(false);
  const [linkType, setLinkType] = useState("relates");
  const [target, setTarget] = useState("");
  const rows = links.data ?? [];
  if (rows.length === 0 && !canEdit) return null;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const key = target.trim().toUpperCase();
    if (!key) return;
    add.mutate({ link_type: linkType, target_key: key }, {
      onSuccess: () => { setTarget(""); setAdding(false); },
      onError: (error) => toast.error("Couldn't link issue", errorMessage(error)),
    });
  };

  // Group by relationship ("blocks", "is blocked by", …) like a reader expects.
  const groups = new Map<string, typeof rows>();
  for (const link of rows) groups.set(link.label, [...(groups.get(link.label) ?? []), link]);

  return (
    <section>
      <SectionTitle
        title="Linked issues"
        action={canEdit && !adding && (
          <Button size="sm" variant="ghost" onClick={() => setAdding(true)}><Plus className="h-3.5 w-3.5" aria-hidden /> Link issue</Button>
        )}
      />
      {links.isPending ? <Skeleton className="h-9" /> : rows.length === 0 && !adding ? <p className="text-sm text-ink-subtle">No linked issues.</p> : null}
      {[...groups.entries()].map(([label, items]) => (
        <div key={label} className="mb-2">
          <div className="mb-1 text-xs font-semibold text-ink-muted">{label}</div>
          <ul className="divide-y divide-line rounded border border-line">
            {items.map((link) => (
              <li key={link.id} className="flex items-center gap-2.5 px-2.5 py-1.5 hover:bg-sunken">
                <IssueTypeIcon type={link.issue.type} />
                <Link to={`/projects/${link.issue.key.split("-")[0]}/issues/${link.issue.key}`} className="shrink-0 text-sm font-medium text-accent hover:underline">{link.issue.key}</Link>
                <span className="min-w-0 flex-1 truncate text-sm text-ink">{link.issue.summary}</span>
                <StatusLozenge name={link.issue.status_name} category={link.issue.status_category} />
                {canEdit && (
                  <button type="button" aria-label={`Remove link to ${link.issue.key}`} className="rounded p-1 text-ink-subtle hover:bg-line hover:text-ink" onClick={() => remove.mutate(link.id, { onError: (error) => toast.error("Couldn't remove link", errorMessage(error)) })}>
                    <X className="h-3.5 w-3.5" />
                  </button>
                )}
              </li>
            ))}
          </ul>
        </div>
      ))}
      {adding && (
        <form onSubmit={submit} className="flex flex-wrap items-center gap-2">
          <div className="w-44"><Select aria-label="Relationship" value={linkType} onValueChange={setLinkType} options={(meta.data?.link_types ?? []).map((t) => ({ value: t.key, label: t.outward }))} /></div>
          <Input autoFocus aria-label="Issue key" placeholder="Issue key, e.g. PAY-12" value={target} onChange={(e) => setTarget(e.target.value)} className="w-48 font-mono" />
          <Button type="submit" variant="primary" loading={add.isPending} disabled={!target.trim()}>Link</Button>
          <Button variant="ghost" onClick={() => setAdding(false)}>Cancel</Button>
        </form>
      )}
    </section>
  );
}
