import { Link } from "react-router-dom";

import { IssueTypeIcon, PriorityIcon, StatusLozenge } from "@/modules/issues";
import type { SearchHit, TextPart } from "@/services/api";
import { formatRelative, titleCase } from "@/utils/format";

/** Text with the matched runs emphasised. Parts are plain text from the API, never markup. */
export function Highlighted({ parts, fallback }: { parts?: TextPart[] | null; fallback: string }) {
  if (!parts?.length) return <>{fallback}</>;
  return (
    <>
      {parts.map((p, i) =>
        p.matched ? (
          <mark key={i} className="rounded-sm bg-warning-soft px-0.5 text-inherit">
            {p.text}
          </mark>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}

export const statusLabel = (status: string) => titleCase(status.replace(/_/g, " "));

export const hitPath = (hit: Pick<SearchHit, "project_key" | "key">) => `/projects/${hit.project_key}/issues/${hit.key}`;

export function SearchResultRow({ hit }: { hit: SearchHit }) {
  return (
    <li>
      <Link to={hitPath(hit)} className="flex gap-3 rounded px-3 py-2.5 hover:bg-sunken focus-visible:bg-sunken focus-visible:outline-none">
        <IssueTypeIcon type={hit.type} className="mt-0.5" />
        <span className="min-w-0 flex-1">
          <span className="flex items-baseline gap-2">
            <span className="shrink-0 text-sm font-medium text-ink-muted">{hit.key}</span>
            <span className="truncate text-sm text-ink">
              <Highlighted parts={hit.summary_parts} fallback={hit.summary} />
            </span>
          </span>
          {hit.snippet && (
            <span className="mt-0.5 line-clamp-2 block text-xs text-ink-muted">
              <span className="text-ink-subtle">{hit.snippet.field === "comments" ? "In comments: " : ""}</span>
              <Highlighted parts={hit.snippet.parts} fallback="" />
            </span>
          )}
        </span>
        <span className="flex shrink-0 items-center gap-3">
          <PriorityIcon priority={hit.priority} />
          <StatusLozenge name={hit.status_name ?? statusLabel(hit.status)} category={hit.status_category} />
          <span className="hidden w-28 whitespace-nowrap text-right text-xs text-ink-subtle sm:inline">{formatRelative(hit.updated_at)}</span>
        </span>
      </Link>
    </li>
  );
}
