import type { ReactNode } from "react";
import { Link } from "react-router-dom";

import { MultiSelect, Select } from "@/components/ui";
import { UserChip, useUserDirectory } from "@/modules/organizations";
import { useComponents, useVersions } from "@/modules/projects";
import type { IssueDetail } from "@/services/api";
import { formatDateTime, titleCase } from "@/utils/format";

import { applicableFields, useCustomFields, useIssueMeta } from "../api";
import { CustomFieldControl } from "./CustomFieldControl";
import { InlineInput } from "./InlineInput";
import { useIssueEdit } from "./useIssueEdit";
import { IssueTypeIcon, PriorityIcon } from "./visuals";

const UNASSIGNED = "unassigned";

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[7.5rem_1fr] items-center gap-2 py-1.5">
      <dt className="text-sm font-medium text-ink-muted">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}

/** Right-hand panel of the issue view: every field is edited in place. */
export function IssueDetails({ issue, canEdit }: { issue: IssueDetail; canEdit: boolean }) {
  const meta = useIssueMeta();
  const { members } = useUserDirectory();
  const { save } = useIssueEdit(issue.key);
  const assignable = members.filter((m) => m.status === "active" || m.user_id === issue.assignee_id);
  const level = meta.data?.types.find((t) => t.key === issue.type)?.level ?? 0;
  const components = useComponents(issue.project_key);
  const versions = useVersions(issue.project_key);
  const fields = applicableFields(useCustomFields().data, issue.project_key, issue.type);
  const customValues = issue.custom_fields as Record<string, unknown>;
  const quiet = "border-transparent shadow-none hover:bg-sunken";

  return (
    <section className="rounded border border-line" aria-label="Details">
      <h2 className="border-b border-line px-3 py-2.5 text-sm font-semibold text-ink">Details</h2>
      <dl className="px-3 py-2">
        <Row label="Assignee">
          {canEdit ? (
            <Select
              size="sm"
              aria-label="Assignee"
              className="border-transparent shadow-none hover:bg-sunken"
              value={issue.assignee_id ?? UNASSIGNED}
              onValueChange={(v) => save({ assignee_id: v === UNASSIGNED ? null : v })}
              options={[{ value: UNASSIGNED, label: "Unassigned" }, ...assignable.map((m) => ({ value: m.user_id, label: m.display_name }))]}
            />
          ) : (
            <UserChip userId={issue.assignee_id} />
          )}
        </Row>
        <Row label="Reporter"><div className="px-2"><UserChip userId={issue.reporter_id} /></div></Row>
        <Row label="Priority">
          {canEdit ? (
            <Select size="sm" aria-label="Priority" className="border-transparent shadow-none hover:bg-sunken" value={issue.priority} onValueChange={(v) => save({ priority: v })} options={(meta.data?.priorities ?? [issue.priority]).map((p) => ({ value: p, label: titleCase(p) }))} />
          ) : (
            <div className="px-2"><PriorityIcon priority={issue.priority} withLabel /></div>
          )}
        </Row>
        <Row label="Type">
          {canEdit ? (
            <Select
              size="sm"
              aria-label="Issue type"
              className="border-transparent shadow-none hover:bg-sunken"
              value={issue.type}
              onValueChange={(v) => save({ type: v })}
              // Only types at the same hierarchy level: moving between levels changes parents.
              options={(meta.data?.types ?? []).filter((t) => t.level === level).map((t) => ({ value: t.key, label: t.name }))}
            />
          ) : (
            <span className="flex items-center gap-2 px-2 text-sm"><IssueTypeIcon type={issue.type} /> {titleCase(issue.type)}</span>
          )}
        </Row>
        {issue.parent && (
          <Row label="Parent">
            <Link to={`/projects/${issue.project_key}/issues/${issue.parent.key}`} className="flex items-center gap-2 px-2 text-sm text-accent hover:underline">
              <IssueTypeIcon type={issue.parent.type} /> <span className="truncate">{issue.parent.key} {issue.parent.summary}</span>
            </Link>
          </Row>
        )}
        <Row label="Labels">
          <InlineInput label="Labels" disabled={!canEdit} placeholder="None" value={issue.labels.join(" ")} onSave={(v) => save({ labels: v.split(/[\s,]+/).filter(Boolean) })} />
        </Row>
        <Row label="Components">
          <MultiSelect aria-label="Components" disabled={!canEdit} className={quiet} values={issue.component_ids}
            onChange={(ids) => save({ component_ids: ids })} options={(components.data ?? []).map((c) => ({ value: c.id, label: c.name }))} />
        </Row>
        <Row label="Fix versions">
          <MultiSelect aria-label="Fix versions" disabled={!canEdit} className={quiet} values={issue.fix_version_ids}
            onChange={(ids) => save({ fix_version_ids: ids })} options={(versions.data ?? []).map((v) => ({ value: v.id, label: v.name }))} />
        </Row>
        <Row label="Story points">
          <InlineInput
            label="Story points"
            type="number"
            disabled={!canEdit}
            placeholder="None"
            value={issue.story_points?.toString() ?? ""}
            onSave={(v) => save({ story_points: v.trim() === "" ? null : Number(v) })}
          />
        </Row>
        <Row label="Start date">
          <InlineInput label="Start date" type="date" disabled={!canEdit} value={issue.start_date ?? ""} onSave={(v) => save({ start_date: v || null })} />
        </Row>
        <Row label="Due date">
          <InlineInput label="Due date" type="date" disabled={!canEdit} value={issue.due_date ?? ""} onSave={(v) => save({ due_date: v || null })} />
        </Row>
        {fields.map((field) => (
          <Row key={field.id} label={field.name}>
            <CustomFieldControl field={field} value={customValues[field.id]} disabled={!canEdit} onCommit={(value) => save({ custom_fields: { [field.id]: value } })} />
          </Row>
        ))}
      </dl>
      <div className="border-t border-line px-3 py-2.5 text-xs text-ink-subtle">
        <div>Created {formatDateTime(issue.created_at)}</div>
        <div>Updated {formatDateTime(issue.updated_at)}</div>
        {issue.resolved_at && <div>Resolved {formatDateTime(issue.resolved_at)}</div>}
      </div>
    </section>
  );
}
