import type { AutomationAction, AutomationRuleDefinition } from "@/services/api";
import { titleCase } from "@/utils/format";

export interface Names {
  status: (key: string) => string;
  type: (key: string) => string;
  person: (id: string) => string;
}

const list = (items: string[]) =>
  items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} or ${items[items.length - 1]}`;

export const WEEKDAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"] as const;

/** "09:00 UTC" */
export const utcHour = (hour: number) => `${String(hour).padStart(2, "0")}:00 UTC`;

export function describeTrigger(def: AutomationRuleDefinition, names: Names): string {
  const t = def.trigger;
  switch (t.type) {
    case "issue_created":
      return "When an issue is created";
    case "issue_transitioned":
      return t.to_status ? `When an issue moves to ${names.status(t.to_status)}` : "When an issue changes status";
    case "scheduled":
      return t.frequency === "weekly" && t.weekday
        ? `Every ${WEEKDAYS[t.weekday - 1]} at ${utcHour(t.hour)}`
        : `Every day at ${utcHour(t.hour)}`;
  }
}

export function describeConditions(def: AutomationRuleDefinition, names: Names): string | null {
  const c = def.conditions;
  const parts: string[] = [];
  if (c?.issue_types?.length) parts.push(`type is ${list(c.issue_types.map(names.type))}`);
  if (c?.priorities?.length) parts.push(`priority is ${list(c.priorities.map(titleCase))}`);
  if (c?.labels_any?.length) parts.push(`labelled ${list(c.labels_any)}`);
  if (c?.statuses?.length) parts.push(`status is ${list(c.statuses.map(names.status))}`);
  if (c?.untouched_days) parts.push(`unchanged for ${c.untouched_days} ${c.untouched_days === 1 ? "day" : "days"}`);
  return parts.length ? `if ${parts.join(" and ")}` : null;
}

export function describeAction(action: AutomationAction, names: Names): string {
  switch (action.type) {
    case "assign":
      if (action.assignee.kind === "reporter") return "assign to the reporter";
      if (action.assignee.kind === "unassigned") return "unassign";
      return `assign to ${names.person(action.assignee.user_id)}`;
    case "set_priority":
      return `set priority to ${titleCase(action.priority)}`;
    case "add_labels":
      return `add ${action.labels.length === 1 ? "label" : "labels"} ${action.labels.join(", ")}`;
    case "transition":
      return `move to ${names.status(action.to_status)}`;
    case "add_comment": {
      const text = action.body.replace(/\s+/g, " ");
      return `comment “${text.length > 40 ? `${text.slice(0, 39).trimEnd()}…` : text}”`;
    }
  }
}

/** One readable sentence: "When an issue is created, if type is Bug: assign to the reporter, set priority to High."
 *  Scheduled rules apply to the project's issues that meet the conditions: "Every day at 09:00 UTC, for issues where …". */
export function describeRule(def: AutomationRuleDefinition, names: Names): string {
  const conditions = describeConditions(def, names);
  const scope = def.trigger.type === "scheduled" ? (conditions ? `for issues ${conditions.replace(/^if /, "where ")}` : "for every issue") : conditions;
  const when = [describeTrigger(def, names), scope].filter(Boolean).join(", ");
  return `${when}: ${def.actions.map((a) => describeAction(a, names)).join(", ")}.`;
}
