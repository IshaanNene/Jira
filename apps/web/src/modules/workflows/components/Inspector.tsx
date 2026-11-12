import { Plus, Trash2, X } from "lucide-react";
import type { ReactNode } from "react";

import { Button, Input, MultiSelect, Select } from "@/components/ui";
import type { WorkflowCondition, WorkflowDefinition, WorkflowPostFunction, WorkflowProblem, WorkflowTransition, WorkflowValidator } from "@/services/api";

import { useCustomFieldChoices } from "../api";
import { CATEGORY_LABEL, describeFrom, removeStatus, removeTransition, updateStatus, updateTransition } from "../model";
import type { Selection } from "./Canvas";

interface InspectorProps {
  definition: WorkflowDefinition;
  selection: Selection;
  problems: WorkflowProblem[];
  readOnly: boolean;
  onChange: (definition: WorkflowDefinition) => void;
  onSelect: (selection: Selection) => void;
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="space-y-2">
      <h3 className="text-2xs font-bold uppercase tracking-wide text-ink-subtle">{title}</h3>
      {children}
    </div>
  );
}

function Labeled({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-sm font-medium text-ink">{label}</span>
      {children}
    </label>
  );
}

const BUILT_IN_FIELDS: [string, string][] = [
  ["assignee", "Assignee"], ["description", "Description"], ["due_date", "Due date"], ["start_date", "Start date"],
  ["story_points", "Story points"], ["labels", "Labels"], ["components", "Components"], ["fix_versions", "Fix versions"], ["parent", "Parent"],
];

/** Right-hand panel: properties of whatever is selected on the canvas. */
export function Inspector({ definition, selection, problems, readOnly, onChange, onSelect }: InspectorProps) {
  if (selection?.kind === "status") {
    const status = definition.statuses.find((s) => s.key === selection.key);
    if (status) {
      return (
        <div className="space-y-5">
          <Section title="Status">
            <Labeled label="Name">
              <Input value={status.name} maxLength={40} disabled={readOnly} onChange={(e) => onChange(updateStatus(definition, status.key, { name: e.target.value }))} />
            </Labeled>
            <Labeled label="Category">
              <Select value={status.category} disabled={readOnly} onValueChange={(category) => onChange(updateStatus(definition, status.key, { category }))}
                options={Object.entries(CATEGORY_LABEL).map(([value, label]) => ({ value, label }))} />
            </Labeled>
            <p className="text-xs text-ink-subtle">The category decides the colour of the status and whether issues in it count as done. Key: <code className="font-mono">{status.key}</code></p>
          </Section>
          {!readOnly && (
            <div className="flex flex-wrap gap-2">
              <Button size="sm" disabled={definition.initial === status.key} onClick={() => onChange({ ...definition, initial: status.key })}>
                {definition.initial === status.key ? "New issues start here" : "Make initial status"}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => { onChange(removeStatus(definition, status.key)); onSelect(null); }}>
                <Trash2 className="h-3.5 w-3.5" aria-hidden /> Delete status
              </Button>
            </div>
          )}
        </div>
      );
    }
  }

  if (selection?.kind === "transition") {
    const transition = definition.transitions.find((t) => t.id === selection.id);
    if (transition) return <TransitionEditor definition={definition} transition={transition} readOnly={readOnly} onChange={onChange} onSelect={onSelect} />;
  }

  return (
    <div className="space-y-5">
      {problems.length > 0 && (
        <Section title="Fix before publishing">
          <ul className="space-y-1.5">
            {problems.map((p, i) => (
              <li key={i} className="rounded border border-warning/30 bg-warning-soft px-2.5 py-1.5 text-sm text-warning">{p.message}</li>
            ))}
          </ul>
        </Section>
      )}
      <Section title={`Transitions (${definition.transitions.length})`}>
        {definition.transitions.length === 0 ? (
          <p className="text-sm text-ink-subtle">No transitions yet. Hover a status and use the link button to connect it to another.</p>
        ) : (
          <ul className="divide-y divide-line rounded border border-line">
            {definition.transitions.map((t) => (
              <li key={t.id}>
                <button type="button" onClick={() => onSelect({ kind: "transition", id: t.id })} className="block w-full px-2.5 py-1.5 text-left hover:bg-sunken">
                  <span className="block truncate text-sm font-medium text-ink">{t.name}</span>
                  <span className="block truncate text-xs text-ink-subtle">
                    {describeFrom(definition, t)} → {definition.statuses.find((s) => s.key === t.to)?.name ?? "?"}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Section>
      <p className="text-xs text-ink-subtle">Select a status or a transition to edit it. Drag statuses to arrange the diagram.</p>
    </div>
  );
}

function TransitionEditor({ definition, transition, readOnly, onChange, onSelect }: {
  definition: WorkflowDefinition;
  transition: WorkflowTransition;
  readOnly: boolean;
  onChange: (definition: WorkflowDefinition) => void;
  onSelect: (selection: Selection) => void;
}) {
  const customFields = useCustomFieldChoices();
  const set = (patch: Partial<WorkflowTransition>) => onChange(updateTransition(definition, transition.id, patch));
  const conditions = transition.conditions ?? [];
  const validators = transition.validators ?? [];
  const postFunctions = transition.post_functions ?? [];
  const statusOptions = definition.statuses.map((s) => ({ value: s.key, label: s.name }));
  const fieldOptions = [
    ...BUILT_IN_FIELDS.map(([value, label]) => ({ value, label })),
    ...(customFields.data ?? []).map((f) => ({ value: `custom:${f.id}`, label: f.name })),
  ];
  const fieldLabel = (field: string) => fieldOptions.find((o) => o.value === field)?.label ?? field;

  const conditionText = (c: WorkflowCondition) =>
    c.type === "role_at_least" ? `Project role is at least ${c.role}` : c.type === "is_assignee" ? "Person is the assignee" : "Person is the reporter";
  const validatorText = (v: WorkflowValidator) => (v.type === "field_required" ? `${v.label ?? fieldLabel(v.field)} is filled in` : "All child issues are done");
  const postText = (p: WorkflowPostFunction) =>
    ({ assign_to_current_user: "Assign to the person making the change", assign_to_reporter: "Assign to the reporter", clear_assignee: "Clear the assignee", clear_resolution: "Clear the resolution", set_resolution: "" })[p.type] ||
    `Set resolution to “${p.type === "set_resolution" ? p.value : ""}”`;

  const addCondition = (value: string) => {
    const next: WorkflowCondition = value.startsWith("role:") ? { type: "role_at_least", role: value.slice(5) } : { type: value as "is_assignee" | "is_reporter" };
    set({ conditions: [...conditions, next] });
  };
  const addValidator = (value: string) => {
    const next: WorkflowValidator = value === "children_done" ? { type: "children_done" } : { type: "field_required", field: value, label: fieldLabel(value) };
    set({ validators: [...validators, next] });
  };
  const addPostFunction = (value: string) => {
    const next: WorkflowPostFunction = value.startsWith("resolution:")
      ? { type: "set_resolution", value: value.slice(11) }
      : { type: value as "assign_to_current_user" | "assign_to_reporter" | "clear_assignee" | "clear_resolution" };
    set({ post_functions: [...postFunctions, next] });
  };

  return (
    <div className="space-y-5">
      <Section title="Transition">
        <Labeled label="Name">
          <Input value={transition.name} maxLength={40} disabled={readOnly} onChange={(e) => set({ name: e.target.value })} />
        </Labeled>
        <div className="space-y-1">
          <span className="text-sm font-medium text-ink">From</span>
          <MultiSelect aria-label="From statuses" placeholder="Any status" disabled={readOnly} values={transition.from ?? []} onChange={(from) => set({ from })} options={statusOptions.filter((o) => o.value !== transition.to)} />
        </div>
        <Labeled label="To">
          <Select value={transition.to} disabled={readOnly} onValueChange={(to) => set({ to, from: (transition.from ?? []).filter((f) => f !== to) })} options={statusOptions} />
        </Labeled>
      </Section>

      <RuleList
        title="Who can do this"
        empty="Anyone who can edit issues in the project."
        items={conditions.map(conditionText)}
        readOnly={readOnly}
        onRemove={(i) => set({ conditions: conditions.filter((_, n) => n !== i) })}
        addLabel="Add a condition"
        options={[
          { value: "role:administrator", label: "Only project administrators" },
          { value: "role:member", label: "Project members and administrators" },
          { value: "is_assignee", label: "Only the assignee" },
          { value: "is_reporter", label: "Only the reporter" },
        ]}
        onAdd={addCondition}
      />
      <RuleList
        title="What must be true"
        empty="Nothing is checked."
        items={validators.map(validatorText)}
        readOnly={readOnly}
        onRemove={(i) => set({ validators: validators.filter((_, n) => n !== i) })}
        addLabel="Add a check"
        options={[...fieldOptions.map((o) => ({ value: o.value, label: `${o.label} is filled in` })), { value: "children_done", label: "All child issues are done" }]}
        onAdd={addValidator}
      />
      <RuleList
        title="What happens afterwards"
        empty="Only the status changes."
        items={postFunctions.map(postText)}
        readOnly={readOnly}
        onRemove={(i) => set({ post_functions: postFunctions.filter((_, n) => n !== i) })}
        addLabel="Add an action"
        options={[
          { value: "assign_to_current_user", label: "Assign to the person making the change" },
          { value: "assign_to_reporter", label: "Assign to the reporter" },
          { value: "clear_assignee", label: "Clear the assignee" },
          { value: "resolution:done", label: "Set resolution to “done”" },
          { value: "resolution:fixed", label: "Set resolution to “fixed”" },
          { value: "resolution:wont_do", label: "Set resolution to “wont_do”" },
          { value: "resolution:duplicate", label: "Set resolution to “duplicate”" },
          { value: "clear_resolution", label: "Clear the resolution" },
        ]}
        onAdd={addPostFunction}
      />

      {!readOnly && (
        <Button size="sm" variant="ghost" onClick={() => { onChange(removeTransition(definition, transition.id)); onSelect(null); }}>
          <Trash2 className="h-3.5 w-3.5" aria-hidden /> Delete transition
        </Button>
      )}
    </div>
  );
}

const PICK = "__pick__";

function RuleList({ title, empty, items, readOnly, onRemove, addLabel, options, onAdd }: {
  title: string;
  empty: string;
  items: string[];
  readOnly: boolean;
  onRemove: (index: number) => void;
  addLabel: string;
  options: { value: string; label: string }[];
  onAdd: (value: string) => void;
}) {
  return (
    <Section title={title}>
      {items.length === 0 ? (
        <p className="text-sm text-ink-subtle">{empty}</p>
      ) : (
        <ul className="space-y-1">
          {items.map((text, i) => (
            <li key={`${text}-${i}`} className="flex items-center gap-2 rounded bg-sunken px-2 py-1 text-sm text-ink">
              <span className="min-w-0 flex-1">{text}</span>
              {!readOnly && (
                <button type="button" aria-label={`Remove: ${text}`} onClick={() => onRemove(i)} className="rounded p-0.5 text-ink-subtle hover:text-danger">
                  <X className="h-3.5 w-3.5" />
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {!readOnly && (
        // A select that always snaps back to its prompt: choosing an option adds a rule.
        <Select size="sm" aria-label={addLabel} value={PICK} onValueChange={(v) => v !== PICK && onAdd(v)}
          options={[{ value: PICK, label: `+ ${addLabel}` }, ...options]} />
      )}
      {!readOnly && items.length === 0 && <span className="sr-only"><Plus aria-hidden /></span>}
    </Section>
  );
}
