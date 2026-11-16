import type { AutomationAction, AutomationRuleDefinition } from "@/services/api";
import { titleCase } from "@/utils/format";

export interface Names {
  status: (key: string) => string;
  type: (key: string) => string;
  person: (id: string) => string;
}

const list = (items: string[]) =>
  items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} or ${items[items.length - 1]}`;

export function describeTrigger(def: AutomationRuleDefinition, names: Names): string {
  if (def.trigger.type === "issue_created") return "When an issue is created";
  const to = def.trigger.to_status;
  return to ? `When an issue moves to ${names.status(to)}` : "When an issue changes status";
}

export function describeConditions(def: AutomationRuleDefinition, names: Names): string | null {
  const c = def.conditions;
  const parts: string[] = [];
  if (c?.issue_types?.length) parts.push(`type is ${list(c.issue_types.map(names.type))}`);
  if (c?.priorities?.length) parts.push(`priority is ${list(c.priorities.map(titleCase))}`);
  if (c?.labels_any?.length) parts.push(`labelled ${list(c.labels_any)}`);
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
  }
}

/** One readable sentence: "When an issue is created, if type is Bug: assign to the reporter, set priority to High." */
export function describeRule(def: AutomationRuleDefinition, names: Names): string {
  const when = [describeTrigger(def, names), describeConditions(def, names)].filter(Boolean).join(", ");
  return `${when}: ${def.actions.map((a) => describeAction(a, names)).join(", ")}.`;
}
