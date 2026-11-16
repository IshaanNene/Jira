import { GitBranch, GitCommitHorizontal, GitPullRequest } from "lucide-react";

import { Badge, ErrorState, Skeleton } from "@/components/ui";
import type { DevLink } from "@/services/api";
import { formatRelative } from "@/utils/format";

import { useDevelopment } from "../api";

const PR_STATE = {
  open: { label: "Open", tone: "success" },
  merged: { label: "Merged", tone: "accent" },
  closed: { label: "Closed", tone: "neutral" },
} as const;

const ICON = { commit: GitCommitHorizontal, branch: GitBranch, pull_request: GitPullRequest } as const;

function label(link: DevLink): string {
  if (link.kind === "commit") return link.ref.slice(0, 7);
  if (link.kind === "pull_request") return `#${link.ref}`;
  return link.ref;
}

/**
 * Commits, branches and pull requests that mention the issue, from connected repositories.
 * Shows nothing at all when no repository is connected to the issue's project.
 */
export function DevelopmentPanel({ issueKey }: { issueKey: string }) {
  const dev = useDevelopment(issueKey);
  if (dev.isPending) return <Skeleton className="h-16" />;
  if (dev.isError) return <ErrorState error={dev.error} onRetry={() => void dev.refetch()} />;
  if (!dev.data.connected) return null;
  const items = dev.data.items;
  return (
    <section aria-label="Development">
      <h2 className="mb-2 text-base font-semibold text-ink">Development</h2>
      {items.length === 0 ? (
        <p className="text-sm text-ink-subtle">No commits, branches or pull requests mention {issueKey} yet.</p>
      ) : (
        <ul className="divide-y divide-line rounded border border-line">
          {items.map((link) => {
            const Icon = ICON[link.kind as keyof typeof ICON] ?? GitCommitHorizontal;
            const state = link.state ? PR_STATE[link.state as keyof typeof PR_STATE] : undefined;
            return (
              <li key={`${link.kind}:${link.repository}:${link.ref}`} className="flex items-start gap-2 px-3 py-2 text-sm">
                <Icon className="mt-0.5 h-4 w-4 shrink-0 text-ink-muted" aria-hidden />
                <div className="min-w-0 flex-1">
                  <a href={link.url} target="_blank" rel="noopener noreferrer" className="text-ink hover:underline">
                    <span className="font-mono text-xs text-ink-muted">{label(link)}</span> {link.title}
                  </a>
                  <p className="text-xs text-ink-subtle">
                    {link.repository}
                    {link.author && ` · ${link.author}`} · {formatRelative(link.occurred_at)}
                  </p>
                </div>
                {state && <Badge tone={state.tone}>{state.label}</Badge>}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
