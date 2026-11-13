import { ChevronRight, Download, Lock, ScrollText, ShieldAlert, ShieldCheck } from "lucide-react";
import { Fragment, useState } from "react";
import { useSearchParams } from "react-router-dom";

import { Avatar, Button, Card, EmptyState, ErrorState, Input, Page, Select, Skeleton, toast } from "@/components/ui";
import { errorMessage, type AuditEntry } from "@/services/api";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";
import { formatDateTime, formatRelative } from "@/utils/format";

import { exportAuditLog, useAuditLog, useMembers, useVerifyAuditLog, type AuditFilters } from "../api";
import { actorName, AUDIT_CATEGORIES, describe } from "../audit";
import { can } from "../permissions";

const ANY = "any";

/** Who changed what in the workspace. Admins and owners only; filters live in the URL. */
export default function AuditLogPage() {
  const role = useSession((s) => s.organization?.role);
  if (!can(role, "viewAuditLog")) {
    return (
      <Page title="Audit log">
        <EmptyState icon={Lock} title="Admins only" description="Ask a workspace admin or owner if you need to see the audit log." />
      </Page>
    );
  }
  return <AuditLog />;
}

function AuditLog() {
  const [url, setUrl] = useSearchParams();
  const filters: AuditFilters = {
    category: url.get("category") ?? undefined,
    actor: url.get("actor") ?? undefined,
    from: url.get("from") ?? undefined,
    to: url.get("to") ?? undefined,
  };
  const set = (key: keyof AuditFilters, value: string | undefined) =>
    setUrl(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (value && value !== ANY) next.set(key, value);
        else next.delete(key);
        return next;
      },
      { replace: true },
    );

  const log = useAuditLog(filters);
  const members = useMembers("");
  const people = Object.assign({}, ...(log.data?.pages.map((p) => p.people) ?? [])) as Record<string, string>;
  const entries = log.data?.pages.flatMap((p) => p.items) ?? [];
  const [exporting, setExporting] = useState(false);
  const filtered = Object.values(filters).some(Boolean);

  const download = async () => {
    setExporting(true);
    try {
      const blob = await exportAuditLog(filters);
      const href = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = href;
      link.download = `audit-log-${new Date().toISOString().slice(0, 10)}.csv`;
      link.click();
      URL.revokeObjectURL(href);
    } catch (error) {
      toast.error("Couldn't export the audit log", errorMessage(error));
    } finally {
      setExporting(false);
    }
  };

  return (
    <Page
      title="Audit log"
      description="Who changed people, access, projects and workflows in this workspace. Entries cannot be edited or deleted."
      width="wide"
      actions={
        <Button variant="secondary" size="sm" onClick={() => void download()} disabled={exporting}>
          <Download className="h-4 w-4" aria-hidden />
          {exporting ? "Exporting…" : "Export CSV"}
        </Button>
      }
    >
      <Integrity />

      <div className="mb-3 flex flex-wrap items-end gap-2">
        <Select
          size="sm"
          aria-label="Activity"
          className="w-52"
          value={filters.category ?? ANY}
          onValueChange={(v) => set("category", v)}
          options={[{ value: ANY, label: "All activity" }, ...AUDIT_CATEGORIES]}
        />
        <Select
          size="sm"
          aria-label="Person"
          className="w-52"
          value={filters.actor ?? ANY}
          onValueChange={(v) => set("actor", v)}
          options={[
            { value: ANY, label: "Anyone" },
            ...(members.data?.pages.flatMap((p) => p.items) ?? []).map((m) => ({ value: m.user_id, label: m.display_name })),
          ]}
        />
        <label className="flex items-center gap-1.5 text-xs text-ink-muted">
          From
          <Input type="date" className="h-8 w-36 text-sm" value={filters.from ?? ""} onChange={(e) => set("from", e.target.value)} />
        </label>
        <label className="flex items-center gap-1.5 text-xs text-ink-muted">
          To
          <Input type="date" className="h-8 w-36 text-sm" value={filters.to ?? ""} onChange={(e) => set("to", e.target.value)} />
        </label>
        {filtered && (
          <Button variant="ghost" size="sm" onClick={() => setUrl(new URLSearchParams(), { replace: true })}>
            Clear filters
          </Button>
        )}
      </div>

      <Card>
        {log.isPending ? (
          <div className="divide-y divide-line" aria-busy aria-label="Loading the audit log">
            {[0, 1, 2, 3, 4].map((i) => (
              <div key={i} className="flex items-center gap-3 px-4 py-3">
                <Skeleton className="h-6 w-6 rounded-full" />
                <Skeleton className="h-3.5 flex-1" />
                <Skeleton className="h-3 w-24" />
              </div>
            ))}
          </div>
        ) : log.isError ? (
          <ErrorState error={log.error} onRetry={() => void log.refetch()} />
        ) : entries.length === 0 ? (
          <EmptyState
            icon={ScrollText}
            title={filtered ? "Nothing matches these filters" : "Nothing recorded yet"}
            description={filtered ? "Try another person, activity or date range." : "Changes to people, access, projects and workflows will appear here."}
          />
        ) : (
          <table className="w-full table-fixed text-left text-sm">
            <thead>
              <tr className="border-b border-line text-2xs font-semibold uppercase tracking-wide text-ink-subtle">
                <th scope="col" className="w-8 px-2 py-2"><span className="sr-only">Details</span></th>
                <th scope="col" className="px-2 py-2 font-semibold">What happened</th>
                <th scope="col" className="hidden w-56 px-2 py-2 font-semibold md:table-cell">Action</th>
                <th scope="col" className="w-40 px-4 py-2 text-right font-semibold">When</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {entries.map((entry) => (
                <EntryRow key={entry.id} entry={entry} people={people} />
              ))}
            </tbody>
          </table>
        )}
        {log.hasNextPage && (
          <div className="flex justify-center border-t border-line p-3">
            <Button variant="secondary" size="sm" onClick={() => void log.fetchNextPage()} disabled={log.isFetchingNextPage}>
              {log.isFetchingNextPage ? "Loading…" : "Show older entries"}
            </Button>
          </div>
        )}
      </Card>
    </Page>
  );
}

function EntryRow({ entry, people }: { entry: AuditEntry; people: Record<string, string> }) {
  const [open, setOpen] = useState(false);
  const actor = actorName(entry, people);
  const facts: [string, string][] = [
    ["Entry", `#${entry.seq}`],
    [entry.resource_type === "user" ? "Person" : `${entry.resource_type[0]?.toUpperCase()}${entry.resource_type.slice(1)}`, entry.resource_id],
    ...Object.entries(entry.details).map(([k, v]): [string, string] => [k.replace(/_/g, " "), k === "user_id" && people[v] ? `${people[v]} (${v})` : v]),
    ["Actor", `${entry.actor.type} ${entry.actor.id}`],
    ...(entry.request_id ? [["Request", entry.request_id] as [string, string]] : []),
  ];
  return (
    <Fragment>
      <tr className="align-top hover:bg-sunken/60">
        <td className="px-2 py-2.5">
          <button
            type="button"
            aria-expanded={open}
            aria-label={open ? "Hide details" : "Show details"}
            onClick={() => setOpen((o) => !o)}
            className="rounded p-0.5 text-ink-subtle hover:bg-sunken hover:text-ink"
          >
            <ChevronRight className={cn("h-4 w-4 transition-transform", open && "rotate-90")} aria-hidden />
          </button>
        </td>
        <td className="px-2 py-2.5">
          <div className="flex items-center gap-2">
            <Avatar name={actor} seed={entry.actor.id} size="sm" />
            <p className="min-w-0 text-ink">
              <span className="font-medium">{actor}</span> {describe(entry, people)}
            </p>
          </div>
        </td>
        <td className="hidden truncate px-2 py-2.5 font-mono text-xs text-ink-muted md:table-cell">{entry.action}</td>
        <td className="whitespace-nowrap px-4 py-2.5 text-right text-xs text-ink-muted" title={formatDateTime(entry.occurred_at)}>
          {formatRelative(entry.occurred_at)}
        </td>
      </tr>
      {open && (
        <tr className="bg-sunken/60">
          <td />
          <td colSpan={3} className="px-2 pb-3 pt-1">
            <dl className="grid grid-cols-[8rem_1fr] gap-x-4 gap-y-1 text-xs">
              <dt className="text-ink-subtle">When</dt>
              <dd className="text-ink">{formatDateTime(entry.occurred_at)}</dd>
              {facts.map(([k, v]) => (
                <Fragment key={k}>
                  <dt className="capitalize text-ink-subtle">{k}</dt>
                  <dd className="break-all font-mono text-ink">{v}</dd>
                </Fragment>
              ))}
            </dl>
          </td>
        </tr>
      )}
    </Fragment>
  );
}

/** Re-checks the hash chain on demand and says plainly what it found. */
function Integrity() {
  const verify = useVerifyAuditLog();
  const result = verify.data;
  return (
    <div
      className={cn(
        "mb-4 flex flex-wrap items-center gap-3 rounded border px-4 py-3 text-sm",
        result && !result.intact ? "border-danger/40 bg-danger-soft" : "border-line bg-surface",
      )}
      role={result && !result.intact ? "alert" : undefined}
    >
      {result && !result.intact ? (
        <ShieldAlert className="h-5 w-5 shrink-0 text-danger" aria-hidden />
      ) : (
        <ShieldCheck className={cn("h-5 w-5 shrink-0", result ? "text-success" : "text-ink-subtle")} aria-hidden />
      )}
      <p className="min-w-0 flex-1 text-ink">
        {verify.isError
          ? `Couldn't check the log: ${errorMessage(verify.error)}`
          : !result
            ? "Every entry is chained to the one before it, so a changed or missing entry can be detected."
            : result.intact
              ? `All ${result.entries} ${result.entries === 1 ? "entry is" : "entries are"} intact.`
              : `Entry #${result.broken_at} failed the check: ${result.reason}. The ${result.entries} entries before it are intact.`}
      </p>
      <Button variant="secondary" size="sm" onClick={() => verify.mutate()} disabled={verify.isPending}>
        {verify.isPending ? "Checking…" : result ? "Check again" : "Check integrity"}
      </Button>
    </div>
  );
}
