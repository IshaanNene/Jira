import { CircleHelp, Search } from "lucide-react";
import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";

import { Button, EmptyState, ErrorState, Input, Page, Select, Skeleton } from "@/components/ui";
import { useProjects } from "@/modules/projects";
import { ApiError, type SavedFilter, type SearchResults } from "@/services/api";
import { cn } from "@/utils/cn";

import { useDebounced, useMqlSearch, useSavedFilter, useSearch, type SearchParams } from "../api";
import { SearchResultRow } from "../components";
import { basicToMql, MQL_EXAMPLES } from "../mql";
import { MqlEditor, type QueryProblem } from "../MqlEditor";
import { SavedFilters } from "../SavedFilters";

const ANY = "any";
const CATEGORIES = [
  { value: ANY, label: "Any status" },
  { value: "todo", label: "To do" },
  { value: "in_progress", label: "In progress" },
  { value: "done", label: "Done" },
];

type Infinite = {
  data?: { pages: SearchResults[] };
  isPending: boolean;
  isError: boolean;
  error: unknown;
  isPlaceholderData: boolean;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  refetch: () => unknown;
  fetchNextPage: () => unknown;
};

/** Issue search across every project the person can browse. State lives in the URL. */
export default function SearchPage() {
  const [url, setUrl] = useSearchParams();
  const mode = url.get("mode") === "mql" ? "mql" : "basic";
  const projects = useProjects();

  const update = (change: Record<string, string | undefined>) =>
    setUrl(
      (prev) => {
        const next = new URLSearchParams(prev);
        for (const [key, value] of Object.entries(change)) {
          if (value && value !== ANY) next.set(key, value);
          else next.delete(key);
        }
        return next;
      },
      { replace: true },
    );

  const switchTo = (next: "basic" | "mql") => {
    if (next === mode) return;
    if (next === "mql") {
      const projectKey = projects.data?.find((p) => p.id === url.get("project"))?.key;
      const mql = url.get("mql") ?? basicToMql({
        q: url.get("q") ?? undefined,
        projectKey,
        statusCategory: url.get("status") ?? undefined,
        mine: url.get("assignee") === "me",
      });
      update({ mode: "mql", mql });
    } else {
      update({ mode: undefined, filter: undefined });
    }
  };

  return (
    <Page
      title="Search issues"
      description={
        mode === "basic"
          ? "Searches keys, summaries, descriptions, comments and labels in every project you can see."
          : "Find issues with a query: fields, operators, AND / OR, and ORDER BY."
      }
      actions={
        <div role="group" aria-label="Search mode" className="inline-flex rounded border border-line-strong p-0.5">
          {(["basic", "mql"] as const).map((m) => (
            <button
              key={m}
              type="button"
              aria-pressed={mode === m}
              onClick={() => switchTo(m)}
              className={cn(
                "rounded-sm px-3 py-1 text-sm font-medium",
                mode === m ? "bg-accent-soft text-accent" : "text-ink-muted hover:text-ink",
              )}
            >
              {m === "basic" ? "Basic" : "MQL"}
            </button>
          ))}
        </div>
      }
    >
      {mode === "basic" ? <BasicSearch url={url} update={update} /> : <QuerySearch url={url} update={update} />}
    </Page>
  );
}

function BasicSearch({ url, update }: { url: URLSearchParams; update: (c: Record<string, string | undefined>) => void }) {
  const [text, setText] = useState(url.get("q") ?? "");
  const q = useDebounced(text, 250);
  const projects = useProjects();
  useEffect(() => update({ q: q.trim() || undefined }), [q]); // eslint-disable-line react-hooks/exhaustive-deps

  const params: SearchParams = {
    q: q.trim(),
    project_id: url.get("project") ?? undefined,
    status_category: url.get("status") ?? undefined,
    assignee: url.get("assignee") ?? undefined,
  };
  const results = useSearch(params);

  return (
    <>
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
          onValueChange={(v) => update({ project: v })}
          options={[{ value: ANY, label: "All projects" }, ...(projects.data ?? []).map((p) => ({ value: p.id, label: p.name }))]}
        />
        <Select size="sm" aria-label="Status" className="w-36" value={params.status_category ?? ANY} onValueChange={(v) => update({ status: v })} options={CATEGORIES} />
        <Button
          size="sm"
          variant={params.assignee === "me" ? "primary" : "secondary"}
          aria-pressed={params.assignee === "me"}
          onClick={() => update({ assignee: params.assignee === "me" ? undefined : "me" })}
        >
          Assigned to me
        </Button>
      </div>
      <Results
        results={results}
        summary={(first) => (params.q ? `${first.total_is_lower_bound ? "More than " : ""}${first.total} results` : "Recently updated")}
        empty={{
          title: params.q ? "No issues match your search" : "No issues yet",
          description: params.q ? "Check the spelling, try fewer words, or clear the filters." : "Issues you can see will appear here.",
        }}
      />
    </>
  );
}

function QuerySearch({ url, update }: { url: URLSearchParams; update: (c: Record<string, string | undefined>) => void }) {
  const filterId = url.get("filter");
  const saved = useSavedFilter(filterId);
  // A link to a filter alone (`?mode=mql&filter=…`) runs the filter's query.
  const ran = url.get("mql") ?? (filterId ? (saved.data?.query ?? "") : "");
  const [draft, setDraft] = useState(ran);
  const [help, setHelp] = useState(false);
  useEffect(() => setDraft(ran), [ran]);
  const results = useMqlSearch(ran, !filterId || url.has("mql") || saved.isSuccess);
  const openFilter = (f: SavedFilter) => update({ filter: f.id, mql: f.query });
  const gone = saved.error instanceof ApiError && saved.error.status === 404;

  const invalid = results.error instanceof ApiError && results.error.status === 422 ? results.error.fieldErrors.find((f) => f.field === "mql") : undefined;
  const problem: QueryProblem | undefined = invalid ? { query: ran, message: invalid.message, span: invalid.span } : undefined;
  const run = (raw: string) => {
    const mql = raw.trim();
    if (mql === ran) void results.refetch();
    else update({ mql: mql || undefined });
  };

  return (
    <>
      {gone ? (
        <p role="alert" className="mb-3 text-sm text-warning">
          That filter was deleted, or it isn't shared with you.{" "}
          <button type="button" className="font-medium underline" onClick={() => update({ filter: undefined })}>
            Dismiss
          </button>
        </p>
      ) : (
        <SavedFilters
          current={saved.data && filterId ? saved.data : null}
          query={ran}
          onOpen={openFilter}
          onClose={() => update({ filter: undefined })}
        />
      )}
      <div className="mb-1">
        <MqlEditor value={draft} onChange={setDraft} onRun={run} problem={problem} />
      </div>
      <div className="mb-4 flex items-center justify-between gap-2 text-xs text-ink-subtle">
        <span>Tab completes a suggestion · Enter searches · Shift+Enter adds a line</span>
        <Button variant="ghost" size="sm" aria-expanded={help} onClick={() => setHelp((h) => !h)}>
          <CircleHelp className="h-3.5 w-3.5" aria-hidden />
          Syntax help
        </Button>
      </div>
      {help && <SyntaxHelp onTry={(q) => { setDraft(q); run(q); }} />}
      {problem ? null : (
        <Results
          results={results}
          summary={(first) => `${first.total_is_lower_bound ? "More than " : ""}${first.total} ${first.total === 1 ? "issue" : "issues"}`}
          empty={{ title: "No issues match this query", description: "Loosen a condition, or check values such as project keys and status names." }}
        />
      )}
    </>
  );
}

function SyntaxHelp({ onTry }: { onTry: (query: string) => void }) {
  const rows: [string, string][] = [
    ["project, type, status, statusCategory", "=  !=  IN (…)  NOT IN (…)"],
    ["priority", "same, plus  <  <=  >  >=  (Lowest … Highest)"],
    ["assignee, reporter", "email, exact name, currentUser(), IS EMPTY"],
    ["labels", "=  !=  IN  IS EMPTY"],
    ["text, summary, description, comment", "~ words   !~ words   (end a word with * for prefixes)"],
    ["created, updated, resolved", "2026-11-01, -7d, -2w, startOfDay(), startOfWeek(), startOfMonth(), now() — UTC"],
  ];
  return (
    <section aria-label="MQL syntax" className="mb-4 rounded border border-line bg-sunken p-4 text-sm">
      <table className="mb-3 w-full text-left">
        <tbody>
          {rows.map(([fields, ops]) => (
            <tr key={fields} className="align-top">
              <th scope="row" className="w-72 py-1 pr-4 font-mono text-xs font-normal text-ink">
                {fields}
              </th>
              <td className="py-1 font-mono text-xs text-ink-muted">{ops}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="mb-1.5 text-xs text-ink-muted">
        Combine with AND, OR, NOT and parentheses. Sort with ORDER BY created, updated, resolved, priority or key (ASC or DESC). Quote values with spaces.
      </p>
      <ul className="space-y-1">
        {MQL_EXAMPLES.map((e) => (
          <li key={e.query} className="flex flex-wrap items-baseline gap-x-3">
            <button type="button" onClick={() => onTry(e.query)} className="font-mono text-xs text-accent hover:underline">
              {e.query}
            </button>
            <span className="text-xs text-ink-subtle">{e.meaning}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Results({
  results,
  summary,
  empty,
}: {
  results: Infinite;
  summary: (first: SearchResults) => string;
  empty: { title: string; description: string };
}) {
  const first = results.data?.pages[0];
  const hits = results.data?.pages.flatMap((p) => p.items) ?? [];
  if (results.isPending) {
    return (
      <div className="space-y-2">
        {[0, 1, 2, 3].map((i) => (
          <Skeleton key={i} className="h-12 w-full" />
        ))}
      </div>
    );
  }
  if (results.isError) return <ErrorState error={results.error} onRetry={() => void results.refetch()} />;
  if (hits.length === 0 || !first) return <EmptyState icon={Search} title={empty.title} description={empty.description} />;
  return (
    <>
      <p className="mb-2 text-xs text-ink-subtle" aria-live="polite">
        {summary(first)}
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
  );
}
