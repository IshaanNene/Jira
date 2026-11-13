import { Search } from "lucide-react";
import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";

import { Button, EmptyState, ErrorState, Input, Page, Select, Skeleton } from "@/components/ui";
import { useProjects } from "@/modules/projects";

import { useDebounced, useSearch, type SearchParams } from "../api";
import { SearchResultRow } from "../components";

const ANY = "any";
const CATEGORIES = [
  { value: ANY, label: "Any status" },
  { value: "todo", label: "To do" },
  { value: "in_progress", label: "In progress" },
  { value: "done", label: "Done" },
];

/** Issue search across every project the person can browse. State lives in the URL. */
export default function SearchPage() {
  const [url, setUrl] = useSearchParams();
  const [text, setText] = useState(url.get("q") ?? "");
  const q = useDebounced(text, 250);
  const projects = useProjects();

  const set = (key: string, value: string | undefined) =>
    setUrl(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (value && value !== ANY) next.set(key, value);
        else next.delete(key);
        return next;
      },
      { replace: true },
    );
  useEffect(() => set("q", q.trim() || undefined), [q]); // eslint-disable-line react-hooks/exhaustive-deps

  const params: SearchParams = {
    q: q.trim(),
    project_id: url.get("project") ?? undefined,
    status_category: url.get("status") ?? undefined,
    assignee: url.get("assignee") ?? undefined,
  };
  const results = useSearch(params);
  const first = results.data?.pages[0];
  const hits = results.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <Page title="Search issues" description="Searches keys, summaries, descriptions, comments and labels in every project you can see.">
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <div className="relative min-w-64 flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-ink-subtle" aria-hidden />
          <Input
            autoFocus
            value={text}
            onChange={(e) => setText(e.target.value)}
            maxLength={200}
            placeholder="Search issues"
            aria-label="Search issues"
            className="pl-8"
          />
        </div>
        <Select
          size="sm"
          aria-label="Project"
          className="w-48"
          value={params.project_id ?? ANY}
          onValueChange={(v) => set("project", v)}
          options={[{ value: ANY, label: "All projects" }, ...(projects.data ?? []).map((p) => ({ value: p.id, label: p.name }))]}
        />
        <Select size="sm" aria-label="Status" className="w-36" value={params.status_category ?? ANY} onValueChange={(v) => set("status", v)} options={CATEGORIES} />
        <Button
          size="sm"
          variant={params.assignee === "me" ? "primary" : "secondary"}
          aria-pressed={params.assignee === "me"}
          onClick={() => set("assignee", params.assignee === "me" ? undefined : "me")}
        >
          Assigned to me
        </Button>
      </div>

      {results.isPending ? (
        <div className="space-y-2">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-12 w-full" />
          ))}
        </div>
      ) : results.isError ? (
        <ErrorState error={results.error} onRetry={() => void results.refetch()} />
      ) : hits.length === 0 ? (
        <EmptyState
          icon={Search}
          title={params.q ? "No issues match your search" : "No issues yet"}
          description={params.q ? "Check the spelling, try fewer words, or clear the filters." : "Issues you can see will appear here."}
        />
      ) : (
        <>
          <p className="mb-2 text-xs text-ink-subtle" aria-live="polite">
            {params.q ? `${first?.total_is_lower_bound ? "More than " : ""}${first?.total} results` : "Recently updated"}
          </p>
          <ul className={results.isPlaceholderData ? "opacity-60" : undefined}>
            {hits.map((hit) => (
              <SearchResultRow key={hit.id} hit={hit} />
            ))}
          </ul>
          {results.hasNextPage && (
            <div className="mt-3 flex justify-center">
              <Button variant="secondary" size="sm" onClick={() => void results.fetchNextPage()} disabled={results.isFetchingNextPage}>
                {results.isFetchingNextPage ? "Loading…" : "Show more"}
              </Button>
            </div>
          )}
        </>
      )}
    </Page>
  );
}
