import { ChevronDown, Eye, FileQuestion, MoreHorizontal } from "lucide-react";
import { useState, type KeyboardEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { Button, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger, EmptyState, ErrorState, Skeleton, toast } from "@/components/ui";
import { roleAtLeast, useProject } from "@/modules/projects";
import { ApiError, errorMessage, type IssueDetail, type RichDoc } from "@/services/api";
import { cn } from "@/utils/cn";

import { useDeleteIssue, useIssue, useTransitions, useWatch } from "../api";
import { Attachments } from "../components/Attachments";
import { IssueActivity } from "../components/IssueActivity";
import { IssueDetails } from "../components/IssueDetails";
import { ChildIssues, LinkedIssues } from "../components/IssueRelations";
import { isEmptyDoc, RichTextEditor, RichTextView } from "../components/RichText";
import { useIssueEdit } from "../components/useIssueEdit";
import { IssueTypeIcon, StatusLozenge } from "../components/visuals";

export default function IssuePage() {
  const { projectKey = "", issueKey = "" } = useParams();
  const key = issueKey.toUpperCase();
  const issue = useIssue(key);
  const project = useProject(projectKey.toUpperCase());

  if (issue.isPending) {
    return (
      <div className="grid gap-8 px-6 py-6 lg:grid-cols-[1fr_22rem]" aria-busy aria-label="Loading issue">
        <div className="space-y-4"><Skeleton className="h-5 w-48" /><Skeleton className="h-9 w-3/4" /><Skeleton className="h-40" /></div>
        <Skeleton className="h-72" />
      </div>
    );
  }
  if (issue.isError) {
    return issue.error instanceof ApiError && issue.error.status === 404 ? (
      <EmptyState className="py-24" icon={FileQuestion} title="Issue not found" description="It may have been deleted, or you may not have access to it."
        action={<Button asChild variant="primary"><Link to={`/projects/${projectKey}/issues`}>Back to issues</Link></Button>} />
    ) : (
      <ErrorState className="py-24" error={issue.error} onRetry={() => void issue.refetch()} />
    );
  }

  const data = issue.data;
  const writable = project.data?.status !== "archived";
  const canEdit = roleAtLeast(data.my_role, "member") && writable;
  return (
    // Keyed by issue so edit drafts never carry over to another issue.
    <div key={data.id} className="px-6 py-5">
      <Header issue={data} projectName={project.data?.name} canDelete={roleAtLeast(data.my_role, "administrator") && writable} />
      <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_22rem]">
        <div className="min-w-0 space-y-7">
          <Summary issue={data} canEdit={canEdit} />
          <Description issue={data} canEdit={canEdit} />
          <Attachments issue={data} canEdit={canEdit} canModerate={roleAtLeast(data.my_role, "administrator")} />
          <ChildIssues issue={data} canEdit={canEdit} />
          <LinkedIssues issue={data} canEdit={canEdit} />
          <IssueActivity issue={data} canComment={canEdit} canModerate={roleAtLeast(data.my_role, "administrator")} />
        </div>
        <aside className="space-y-3">
          <StatusButton issue={data} canEdit={canEdit} />
          <IssueDetails issue={data} canEdit={canEdit} />
        </aside>
      </div>
    </div>
  );
}

function Header({ issue, projectName, canDelete }: { issue: IssueDetail; projectName: string | undefined; canDelete: boolean }) {
  const navigate = useNavigate();
  const watch = useWatch(issue.key);
  const remove = useDeleteIssue(issue.key, issue.project_key);
  const crumb = "text-ink-muted hover:underline";
  return (
    <div className="mb-3 flex items-center justify-between gap-3">
      <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-1.5 text-sm">
        <Link to="/projects" className={crumb}>Projects</Link>
        <span className="text-ink-subtle">/</span>
        <Link to={`/projects/${issue.project_key}/issues`} className={cn(crumb, "truncate")}>{projectName ?? issue.project_key}</Link>
        {issue.parent && (
          <>
            <span className="text-ink-subtle">/</span>
            <Link to={`/projects/${issue.project_key}/issues/${issue.parent.key}`} className={cn(crumb, "flex items-center gap-1.5")}>
              <IssueTypeIcon type={issue.parent.type} /> {issue.parent.key}
            </Link>
          </>
        )}
        <span className="text-ink-subtle">/</span>
        <span className="flex items-center gap-1.5 text-ink-muted" aria-current="page"><IssueTypeIcon type={issue.type} /> {issue.key}</span>
      </nav>
      <div className="flex shrink-0 items-center gap-1">
        <Button
          variant="ghost"
          aria-pressed={issue.watching}
          aria-label={issue.watching ? "Stop watching" : "Watch this issue"}
          title={issue.watching ? "You are watching this issue" : "Watch this issue"}
          className={issue.watching ? "text-accent" : undefined}
          onClick={() => watch.mutate(!issue.watching, { onError: (error) => toast.error("Couldn't update watching", errorMessage(error)) })}
        >
          <Eye className="h-4 w-4" aria-hidden /> <span className="tabular">{issue.watcher_count}</span>
        </Button>
        {canDelete && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="More actions"><MoreHorizontal className="h-4 w-4" /></Button></DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem
                tone="danger"
                onSelect={() => {
                  if (!window.confirm(`Delete ${issue.key}? This cannot be undone from here.`)) return;
                  remove.mutate(undefined, {
                    onSuccess: () => { toast.success(`${issue.key} deleted`); navigate(`/projects/${issue.project_key}/issues`); },
                    onError: (error) => toast.error("Couldn't delete issue", errorMessage(error)),
                  });
                }}
              >
                Delete issue
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </div>
    </div>
  );
}

function Summary({ issue, canEdit }: { issue: IssueDetail; canEdit: boolean }) {
  const { save } = useIssueEdit(issue.key);
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    const next = draft?.trim();
    if (next && next !== issue.summary) save({ summary: next });
    setDraft(null);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      e.currentTarget.blur();
    }
    if (e.key === "Escape") {
      setDraft(null);
      const el = e.currentTarget;
      requestAnimationFrame(() => el.blur());
    }
  };
  if (!canEdit) return <h1 className="text-2xl font-semibold leading-tight text-ink">{issue.summary}</h1>;
  return (
    <h1>
      <textarea
        aria-label="Summary"
        rows={1}
        maxLength={255}
        value={draft ?? issue.summary}
        onChange={(e) => setDraft(e.target.value.replace(/\n/g, " "))}
        onBlur={commit}
        onKeyDown={onKeyDown}
        className="-mx-2 block w-[calc(100%+1rem)] resize-none rounded border border-transparent bg-transparent px-2 py-1 text-2xl font-semibold leading-tight text-ink [field-sizing:content] hover:bg-sunken focus-visible:border-accent focus-visible:bg-surface focus-visible:ring-0"
      />
    </h1>
  );
}

function Description({ issue, canEdit }: { issue: IssueDetail; canEdit: boolean }) {
  const { save, saving } = useIssueEdit(issue.key);
  const [editing, setEditing] = useState(false);
  const stored = (issue.description as RichDoc | null | undefined) ?? null;
  const [draft, setDraft] = useState<RichDoc | null>(stored);
  const empty = isEmptyDoc(stored);

  return (
    <section>
      <h2 className="mb-2 text-base font-semibold text-ink">Description</h2>
      {editing ? (
        <>
          <RichTextEditor aria-label="Description" autoFocus initial={stored} placeholder="Add a description…" onChange={setDraft} minHeight="9rem" />
          <div className="mt-2 flex gap-2">
            <Button variant="primary" loading={saving} onClick={() => save({ description: isEmptyDoc(draft) ? null : draft }, () => setEditing(false))}>Save</Button>
            <Button variant="ghost" onClick={() => { setDraft(stored); setEditing(false); }}>Cancel</Button>
          </div>
        </>
      ) : canEdit ? (
        // The whole block is the edit affordance, as people expect from issue trackers.
        <div
          role="button"
          tabIndex={0}
          aria-label="Edit description"
          onClick={() => { setDraft(stored); setEditing(true); }}
          onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), setDraft(stored), setEditing(true))}
          className="-mx-2 cursor-text rounded px-2 py-1.5 hover:bg-sunken"
        >
          {empty ? <p className="text-base text-ink-subtle">Add a description…</p> : <RichTextView doc={stored!} />}
        </div>
      ) : empty ? (
        <p className="text-base text-ink-subtle">No description.</p>
      ) : (
        <RichTextView doc={stored!} />
      )}
    </section>
  );
}

const STATUS_BUTTON: Record<string, string> = {
  todo: "bg-line text-ink hover:bg-line-strong",
  in_progress: "bg-accent text-accent-ink hover:bg-accent-hover",
  done: "bg-success text-white hover:opacity-90",
};

function StatusButton({ issue, canEdit }: { issue: IssueDetail; canEdit: boolean }) {
  const { save, saving } = useIssueEdit(issue.key);
  const transitions = useTransitions(issue.key, canEdit);
  const classes = cn("inline-flex h-8 items-center gap-1.5 rounded px-3 text-sm font-semibold", STATUS_BUTTON[issue.status_category], saving && "opacity-70");
  if (!canEdit) return <span className={classes}>{issue.status_name}</span>;
  const options = transitions.data ?? [];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className={classes} aria-label={`Status: ${issue.status_name}. Change status`}>
        {issue.status_name} <ChevronDown className="h-3.5 w-3.5" aria-hidden />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[16rem]">
        {transitions.isPending ? (
          <div className="px-2 py-1.5 text-sm text-ink-subtle">Loading…</div>
        ) : transitions.isError ? (
          <div className="px-2 py-1.5 text-sm text-danger">Couldn&rsquo;t load the available steps.</div>
        ) : options.length === 0 ? (
          <div className="px-2 py-1.5 text-sm text-ink-subtle">You can&rsquo;t move this issue from {issue.status_name}.</div>
        ) : (
          // Each entry is a step of the issue's workflow: its name, and where it leads.
          options.map((t) => (
            <DropdownMenuItem key={t.id} onSelect={() => save({ status: t.to.key })}>
              <span className="min-w-0 flex-1 truncate">{t.name}</span>
              <span aria-hidden className="text-ink-subtle">→</span>
              <StatusLozenge name={t.to.name} category={t.to.category} />
            </DropdownMenuItem>
          ))
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
