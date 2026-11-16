import { useState } from "react";

import { Avatar, Button, ErrorState, Skeleton, toast } from "@/components/ui";
import { useUserDirectory } from "@/modules/organizations";
import { useComponents, useVersions } from "@/modules/projects";
import { errorMessage, type HistoryEntry, type IssueComment, type IssueDetail, type RichDoc } from "@/services/api";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";
import { formatDateTime, formatRelative, titleCase } from "@/utils/format";

import { useCommentMutations, useComments, useHistory } from "../api";
import { isEmptyDoc, RichTextEditor, RichTextView } from "./RichText";

export function IssueActivity({ issue, canComment, canModerate }: { issue: IssueDetail; canComment: boolean; canModerate: boolean }) {
  const [tab, setTab] = useState<"comments" | "history">("comments");
  return (
    <section>
      <h2 className="mb-2 text-base font-semibold text-ink">Activity</h2>
      <div className="mb-3 flex items-center gap-1 text-sm" role="tablist" aria-label="Activity">
        <span className="mr-1 text-ink-muted">Show:</span>
        {(["comments", "history"] as const).map((t) => (
          <button key={t} type="button" role="tab" aria-selected={tab === t} onClick={() => setTab(t)}
            className={cn("rounded px-2 py-1 font-medium capitalize", tab === t ? "bg-accent-soft text-accent" : "bg-sunken text-ink-muted hover:text-ink")}>
            {t}
          </button>
        ))}
      </div>
      {tab === "comments" ? <Comments issue={issue} canComment={canComment} canModerate={canModerate} /> : <History issueKey={issue.key} projectKey={issue.project_key} />}
    </section>
  );
}

function Comments({ issue, canComment, canModerate }: { issue: IssueDetail; canComment: boolean; canModerate: boolean }) {
  const me = useSession((s) => s.account);
  const comments = useComments(issue.key);
  const { add } = useCommentMutations(issue.key);
  const [draft, setDraft] = useState<RichDoc | null>(null);
  const [composing, setComposing] = useState(false);
  const [resetKey, setResetKey] = useState(0);

  const post = () => {
    if (!draft || isEmptyDoc(draft)) return;
    add.mutate(draft, {
      onSuccess: () => { setDraft(null); setComposing(false); setResetKey((k) => k + 1); },
      onError: (error) => toast.error("Couldn't post comment", errorMessage(error)),
    });
  };

  return (
    <div className="space-y-4">
      {canComment && (
        <div className="flex gap-3">
          <Avatar name={me?.display_name ?? "?"} seed={me?.id} />
          <div className="min-w-0 flex-1">
            {composing ? (
              <>
                <RichTextEditor aria-label="Comment" autoFocus placeholder="Add a comment… Type @ to mention someone." onChange={setDraft} resetKey={resetKey} minHeight="5rem" />
                <div className="mt-2 flex gap-2">
                  <Button variant="primary" loading={add.isPending} disabled={isEmptyDoc(draft)} onClick={post}>Save</Button>
                  <Button variant="ghost" onClick={() => { setComposing(false); setDraft(null); }}>Cancel</Button>
                </div>
              </>
            ) : (
              <button type="button" onClick={() => setComposing(true)} className="h-9 w-full rounded border border-line-strong px-3 text-left text-base text-ink-subtle hover:bg-sunken">
                Add a comment…
              </button>
            )}
          </div>
        </div>
      )}
      {comments.isPending ? (
        <Skeleton className="h-14" />
      ) : comments.isError ? (
        <ErrorState error={comments.error} onRetry={() => void comments.refetch()} />
      ) : comments.data.length === 0 ? (
        <p className="text-sm text-ink-subtle">No comments yet.</p>
      ) : (
        // Newest first, like the composer above suggests.
        [...comments.data].reverse().map((comment) => <CommentItem key={comment.id} issueKey={issue.key} comment={comment} canModerate={canModerate} />)
      )}
    </div>
  );
}

function CommentItem({ issueKey, comment, canModerate }: { issueKey: string; comment: IssueComment; canModerate: boolean }) {
  const myId = useSession((s) => s.account?.id);
  const { byId } = useUserDirectory();
  const { edit, remove } = useCommentMutations(issueKey);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<RichDoc>(comment.body as RichDoc);
  const mine = comment.author_id === myId;
  const author = byId.get(comment.author_id)?.display_name ?? "Former member";

  return (
    <article className="flex gap-3">
      <Avatar name={author} seed={comment.author_id} />
      <div className="min-w-0 flex-1">
        <header className="flex items-baseline gap-2 text-sm">
          <span className="font-semibold text-ink">{author}</span>
          <time className="text-ink-subtle" dateTime={comment.created_at} title={formatDateTime(comment.created_at)}>{formatRelative(comment.created_at)}</time>
          {comment.edited_at && <span className="text-ink-subtle">· Edited</span>}
        </header>
        {editing ? (
          <div className="mt-1">
            <RichTextEditor aria-label="Edit comment" autoFocus initial={comment.body as RichDoc} onChange={setDraft} minHeight="4rem" />
            <div className="mt-2 flex gap-2">
              <Button variant="primary" loading={edit.isPending} disabled={isEmptyDoc(draft)}
                onClick={() => edit.mutate({ id: comment.id, body: draft }, { onSuccess: () => setEditing(false), onError: (error) => toast.error("Couldn't save comment", errorMessage(error)) })}>
                Save
              </Button>
              <Button variant="ghost" onClick={() => setEditing(false)}>Cancel</Button>
            </div>
          </div>
        ) : (
          <>
            <RichTextView doc={comment.body as RichDoc} className="mt-1" />
            <div className="mt-1 flex gap-3 text-sm font-medium text-ink-muted">
              {mine && <button type="button" className="hover:text-ink hover:underline" onClick={() => setEditing(true)}>Edit</button>}
              {(mine || canModerate) && (
                <button type="button" className="hover:text-ink hover:underline"
                  onClick={() => window.confirm("Delete this comment?") && remove.mutate(comment.id, { onError: (error) => toast.error("Couldn't delete comment", errorMessage(error)) })}>
                  Delete
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </article>
  );
}

function History({ issueKey, projectKey }: { issueKey: string; projectKey: string }) {
  const components = useComponents(projectKey);
  const versions = useVersions(projectKey);
  const history = useHistory(issueKey, true);
  const { byId } = useUserDirectory();
  if (history.isPending) return <Skeleton className="h-14" />;
  if (history.isError) return <ErrorState error={history.error} onRetry={() => void history.refetch()} />;

  const show = (field: string, value: unknown): string => {
    if (value === null || value === undefined || value === "") return "None";
    if (field === "assignee") return byId.get(String(value))?.display_name ?? "Former member";
    if (field === "components" || field === "fix_versions") {
      const source: { id: string; name: string }[] = (field === "components" ? components.data : versions.data) ?? [];
      const ids = value as string[];
      return ids.length ? ids.map((id) => source.find((x) => x.id === id)?.name ?? "Removed").join(", ") : "None";
    }
    if (typeof value === "boolean") return value ? "Yes" : "No";
    if (Array.isArray(value)) return value.length ? value.join(", ") : "None";
    if (field === "priority" || field === "type") return titleCase(String(value));
    return String(value);
  };
  const describe = (entry: HistoryEntry) =>
    (entry.changes as unknown as { field: string; from: unknown; to: unknown; custom?: boolean; rule?: string }[]).map((c, i) => {
      if (c.field === "created") return <div key={i}>created the issue</div>;
      if (c.field === "description") return <div key={i}>updated the <strong className="font-medium">Description</strong></div>;
      if (c.field === "link") return <div key={i}>linked this issue: {String(c.to)}</div>;
      if (c.field === "attachment") return <div key={i}>{c.to ? <>attached <strong className="font-medium">{String(c.to)}</strong></> : <>removed attachment <strong className="font-medium">{String(c.from)}</strong></>}</div>;
      return (
        <div key={i}>
          changed <strong className="font-medium">{c.custom ? c.field : titleCase(c.field)}</strong>{" "}
          <span className="text-ink-muted">{show(c.field, c.from)}</span> → <span>{show(c.field, c.to)}</span>
          {c.rule && <span className="text-ink-subtle"> · by automation rule “{c.rule}”</span>}
        </div>
      );
    });

  return (
    <ol className="space-y-3">
      {history.data.map((entry) => {
        const actor = byId.get(entry.actor_id)?.display_name ?? "Former member";
        return (
          <li key={entry.id} className="flex gap-3">
            <Avatar name={actor} seed={entry.actor_id} />
            <div className="min-w-0 text-sm text-ink">
              <span className="font-semibold">{actor}</span>{" "}
              <time className="text-ink-subtle" dateTime={entry.created_at} title={formatDateTime(entry.created_at)}>{formatRelative(entry.created_at)}</time>
              {describe(entry)}
            </div>
          </li>
        );
      })}
    </ol>
  );
}
