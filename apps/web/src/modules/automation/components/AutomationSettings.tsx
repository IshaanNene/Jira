import { History, Plus, Trash2, Zap } from "lucide-react";
import { useMemo, useState, type FormEvent } from "react";
import { Link } from "react-router-dom";

import {
  Badge,
  Button,
  Card,
  CardHeader,
  Dialog,
  DialogContent,
  DialogFooter,
  EmptyState,
  ErrorState,
  Field,
  Input,
  MultiSelect,
  Select,
  Skeleton,
  toast,
} from "@/components/ui";
import { useIssueMeta, useProjectStatuses } from "@/modules/issues";
import { useUserDirectory } from "@/modules/organizations";
import { ApiError, errorMessage, type AutomationAction, type AutomationRule, type AutomationRuleDefinition } from "@/services/api";
import { formatRelative, titleCase } from "@/utils/format";

import { useAutomationMutations, useAutomationRules, useAutomationRuns, type RuleInput } from "../api";
import { describeRule, type Names } from "../describe";

const MAX_ACTIONS = 5;
const ANY_STATUS = "__any__";

function useNames(projectId: string): Names & { statuses: { key: string; name: string }[]; types: { key: string; name: string }[] } {
  const statuses = useProjectStatuses(projectId);
  const meta = useIssueMeta();
  const { byId } = useUserDirectory();
  return useMemo(() => {
    const s = statuses.data ?? [];
    const t = meta.data?.types ?? [];
    return {
      statuses: s,
      types: t,
      status: (key) => s.find((x) => x.key === key)?.name ?? titleCase(key),
      type: (key) => t.find((x) => x.key === key)?.name ?? titleCase(key),
      person: (id) => byId.get(id)?.display_name ?? "a former member",
    };
  }, [statuses.data, meta.data, byId]);
}

/** Project settings → Automation: declarative rules that act on issues as they are created or moved. */
export function AutomationSettings({ projectKey, projectId }: { projectKey: string; projectId: string }) {
  const rules = useAutomationRules(projectKey);
  const { update, remove } = useAutomationMutations(projectKey);
  const names = useNames(projectId);
  const [editing, setEditing] = useState<AutomationRule | "new" | null>(null);
  const [runsOf, setRunsOf] = useState<AutomationRule | null>(null);

  const toggle = (rule: AutomationRule) =>
    update.mutate(
      { rule, input: { name: rule.name, enabled: !rule.enabled, definition: rule.definition } },
      { onError: (e) => toast.error(rule.enabled ? "Couldn't turn the rule off" : "Couldn't turn the rule on", errorMessage(e)) },
    );

  return (
    <Card>
      <CardHeader
        title="Automation"
        description="Rules that act on issues for you as they are created or change status. They run as the person who made the change, in the same step."
        actions={<Button variant="primary" onClick={() => setEditing("new")} disabled={(rules.data?.length ?? 0) >= 50}><Plus className="h-4 w-4" aria-hidden /> New rule</Button>}
      />
      {rules.isPending ? (
        <div className="p-4" aria-busy><Skeleton className="h-12" /></div>
      ) : rules.isError ? (
        <ErrorState error={rules.error} onRetry={() => void rules.refetch()} />
      ) : rules.data.length === 0 ? (
        <EmptyState
          icon={Zap}
          title="No rules yet"
          description="For example: when a bug is created, assign it to the reporter and set its priority to High."
        />
      ) : (
        <ul className="divide-y divide-line" aria-label="Automation rules">
          {rules.data.map((rule) => (
            <li key={rule.id} className="flex flex-wrap items-start gap-3 px-4 py-3">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="font-medium text-ink">{rule.name}</span>
                  {!rule.enabled && <Badge>Off</Badge>}
                </div>
                <p className="mt-0.5 text-sm text-ink-muted">{describeRule(rule.definition, names)}</p>
              </div>
              <label className="flex items-center gap-2 text-sm text-ink-muted">
                <input type="checkbox" role="switch" checked={rule.enabled} onChange={() => toggle(rule)} aria-label={`${rule.name} enabled`} className="h-4 w-4 accent-[hsl(var(--accent))]" />
                On
              </label>
              <Button size="sm" variant="ghost" onClick={() => setEditing(rule)}>Edit</Button>
              <Button size="sm" variant="ghost" aria-label={`Recent runs of ${rule.name}`} onClick={() => setRunsOf(rule)}><History className="h-4 w-4" aria-hidden /></Button>
              <Button
                size="sm"
                variant="ghost"
                aria-label={`Delete ${rule.name}`}
                onClick={() => {
                  if (window.confirm(`Delete the rule "${rule.name}"? Its run history goes with it.`)) {
                    remove.mutate(rule, { onError: (e) => toast.error("Couldn't delete the rule", errorMessage(e)) });
                  }
                }}
              >
                <Trash2 className="h-4 w-4" aria-hidden />
              </Button>
            </li>
          ))}
        </ul>
      )}
      {editing && <RuleDialog projectKey={projectKey} names={names} rule={editing === "new" ? null : editing} onClose={() => setEditing(null)} />}
      {runsOf && <RunsDialog projectKey={projectKey} rule={runsOf} onClose={() => setRunsOf(null)} />}
    </Card>
  );
}

const BLANK: AutomationRuleDefinition = {
  trigger: { type: "issue_created" },
  conditions: { issue_types: [], priorities: [], labels_any: [] },
  actions: [{ type: "set_priority", priority: "high" }],
};

function defaultAction(type: AutomationAction["type"]): AutomationAction {
  if (type === "assign") return { type, assignee: { kind: "reporter" } };
  if (type === "set_priority") return { type, priority: "high" };
  return { type, labels: [] };
}

const splitLabels = (text: string) => text.split(/[\s,]+/).map((l) => l.trim()).filter(Boolean);

function RuleDialog({ projectKey, names, rule, onClose }: { projectKey: string; names: ReturnType<typeof useNames>; rule: AutomationRule | null; onClose: () => void }) {
  const { create, update } = useAutomationMutations(projectKey);
  const meta = useIssueMeta();
  const { members } = useUserDirectory();
  const [name, setName] = useState(rule?.name ?? "");
  const [def, setDef] = useState<AutomationRuleDefinition>(rule?.definition ?? BLANK);
  const [labelText, setLabelText] = useState((rule?.definition.conditions?.labels_any ?? []).join(", "));
  const [actionLabels, setActionLabels] = useState<string[]>(def.actions.map((a) => (a.type === "add_labels" ? a.labels.join(", ") : "")));
  const [error, setError] = useState<string | null>(null);
  const priorities = (meta.data?.priorities ?? ["highest", "high", "medium", "low", "lowest"]).map((p) => ({ value: p, label: titleCase(p) }));
  const people = members.filter((m) => m.status === "active").map((m) => ({ value: m.user_id, label: m.display_name }));

  const trigger = def.trigger.type === "issue_created" ? "issue_created" : "issue_transitioned";
  const toStatus = def.trigger.type === "issue_transitioned" ? (def.trigger.to_status ?? ANY_STATUS) : ANY_STATUS;
  const setAction = (i: number, action: AutomationAction) => setDef({ ...def, actions: def.actions.map((a, j) => (j === i ? action : a)) });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    const definition: AutomationRuleDefinition = {
      ...def,
      conditions: { ...def.conditions, labels_any: splitLabels(labelText) },
      actions: def.actions.map((a, i) => (a.type === "add_labels" ? { ...a, labels: splitLabels(actionLabels[i] ?? "") } : a)),
    };
    const input: RuleInput = { name: name.trim(), enabled: rule?.enabled ?? true, definition };
    const done = { onSuccess: onClose, onError: (err: unknown) => setError(err instanceof ApiError ? errorMessage(err) : "Couldn't save the rule.") };
    if (rule) update.mutate({ rule, input }, done);
    else create.mutate(input, done);
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={rule ? "Edit rule" : "New rule"} description="A trigger, optional conditions, and up to five actions." className="max-w-xl">
        <form onSubmit={submit} className="space-y-4">
          <Field label="Name">
            <Input value={name} maxLength={100} onChange={(e) => setName(e.target.value)} placeholder="e.g. Triage new bugs" />
          </Field>
          <fieldset className="space-y-2">
            <legend className="text-sm font-semibold text-ink">When</legend>
            <div className="grid gap-2 sm:grid-cols-2">
              <Select
                aria-label="Trigger"
                value={trigger}
                onValueChange={(v) => setDef({ ...def, trigger: v === "issue_created" ? { type: "issue_created" } : { type: "issue_transitioned" } })}
                options={[{ value: "issue_created", label: "An issue is created" }, { value: "issue_transitioned", label: "An issue changes status" }]}
              />
              {trigger === "issue_transitioned" && (
                <Select
                  aria-label="To status"
                  value={toStatus}
                  onValueChange={(v) => setDef({ ...def, trigger: { type: "issue_transitioned", to_status: v === ANY_STATUS ? null : v } })}
                  options={[{ value: ANY_STATUS, label: "To any status" }, ...names.statuses.map((s) => ({ value: s.key, label: `To ${s.name}` }))]}
                />
              )}
            </div>
          </fieldset>
          <fieldset className="space-y-2">
            <legend className="text-sm font-semibold text-ink">If <span className="font-normal text-ink-muted">(all optional)</span></legend>
            <div className="grid gap-2 sm:grid-cols-2">
              <MultiSelect
                aria-label="Issue types"
                placeholder="Any type"
                values={def.conditions?.issue_types ?? []}
                onChange={(v) => setDef({ ...def, conditions: { ...def.conditions, issue_types: v } })}
                options={names.types.map((t) => ({ value: t.key, label: t.name }))}
              />
              <MultiSelect
                aria-label="Priorities"
                placeholder="Any priority"
                values={def.conditions?.priorities ?? []}
                onChange={(v) => setDef({ ...def, conditions: { ...def.conditions, priorities: v } })}
                options={priorities}
              />
            </div>
            <Input aria-label="Has any of these labels" placeholder="Has any of these labels (comma-separated)" value={labelText} onChange={(e) => setLabelText(e.target.value)} />
          </fieldset>
          <fieldset className="space-y-2">
            <legend className="text-sm font-semibold text-ink">Then</legend>
            <ol className="space-y-2">
              {def.actions.map((action, i) => (
                <li key={i} className="flex gap-2">
                  <Select
                    aria-label={`Action ${i + 1}`}
                    className="w-44 shrink-0"
                    value={action.type}
                    onValueChange={(v) => setAction(i, defaultAction(v as AutomationAction["type"]))}
                    options={[{ value: "assign", label: "Assign" }, { value: "set_priority", label: "Set priority" }, { value: "add_labels", label: "Add labels" }]}
                  />
                  {action.type === "assign" && (
                    <Select
                      aria-label={`Action ${i + 1} assignee`}
                      value={action.assignee.kind === "user" ? action.assignee.user_id : action.assignee.kind}
                      onValueChange={(v) =>
                        setAction(i, { type: "assign", assignee: v === "reporter" || v === "unassigned" ? { kind: v } : { kind: "user", user_id: v } })
                      }
                      options={[{ value: "reporter", label: "The reporter" }, { value: "unassigned", label: "Nobody (unassign)" }, ...people]}
                    />
                  )}
                  {action.type === "set_priority" && (
                    <Select aria-label={`Action ${i + 1} priority`} value={action.priority} onValueChange={(v) => setAction(i, { type: "set_priority", priority: v })} options={priorities} />
                  )}
                  {action.type === "add_labels" && (
                    <Input
                      aria-label={`Action ${i + 1} labels`}
                      placeholder="Labels, comma-separated"
                      value={actionLabels[i] ?? ""}
                      onChange={(e) => setActionLabels(actionLabels.map((l, j) => (j === i ? e.target.value : l)).concat(i >= actionLabels.length ? [e.target.value] : []))}
                    />
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Remove action ${i + 1}`}
                    disabled={def.actions.length === 1}
                    onClick={() => {
                      setDef({ ...def, actions: def.actions.filter((_, j) => j !== i) });
                      setActionLabels(actionLabels.filter((_, j) => j !== i));
                    }}
                  >
                    <Trash2 className="h-4 w-4" aria-hidden />
                  </Button>
                </li>
              ))}
            </ol>
            {def.actions.length < MAX_ACTIONS && (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={() => {
                  setDef({ ...def, actions: [...def.actions, defaultAction("add_labels")] });
                  setActionLabels([...actionLabels, ""]);
                }}
              >
                <Plus className="h-4 w-4" aria-hidden /> Add action
              </Button>
            )}
          </fieldset>
          {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={!name.trim()} loading={create.isPending || update.isPending}>{rule ? "Save rule" : "Create rule"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

const OUTCOME = {
  applied: { label: "Applied", tone: "success" },
  no_change: { label: "Nothing to change", tone: "neutral" },
  failed: { label: "Skipped", tone: "danger" },
} as const;

function RunsDialog({ projectKey, rule, onClose }: { projectKey: string; rule: AutomationRule; onClose: () => void }) {
  const runs = useAutomationRuns(rule.id);
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={`Recent runs: ${rule.name}`} description="The latest 100, newest first." className="max-w-lg">
        {runs.isPending ? (
          <Skeleton className="h-16" />
        ) : runs.isError ? (
          <ErrorState error={runs.error} onRetry={() => void runs.refetch()} />
        ) : runs.data.length === 0 ? (
          <p className="text-sm text-ink-muted">This rule hasn&apos;t run yet.</p>
        ) : (
          <ul className="max-h-80 divide-y divide-line overflow-auto" aria-label="Runs">
            {runs.data.map((run) => {
              const outcome = OUTCOME[run.outcome as keyof typeof OUTCOME] ?? OUTCOME.no_change;
              return (
                <li key={run.id} className="py-2 text-sm">
                  <div className="flex items-center gap-2">
                    <Link to={`/projects/${projectKey}/issues/${run.issue_key}`} className="font-medium text-accent hover:underline">{run.issue_key}</Link>
                    <Badge tone={outcome.tone}>{outcome.label}</Badge>
                    <span className="ml-auto text-xs text-ink-subtle">{formatRelative(run.created_at)}</span>
                  </div>
                  {run.detail && <p className="mt-0.5 text-ink-muted">{run.detail}</p>}
                </li>
              );
            })}
          </ul>
        )}
      </DialogContent>
    </Dialog>
  );
}
