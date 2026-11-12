import type { WorkflowDefinition, WorkflowStatus, WorkflowTransition } from "@/services/api";

// Pure editing operations on a workflow definition. Every function returns a new
// definition; the designer keeps one working copy in component state and the server
// validates it again on save and publish.

export const NODE_WIDTH = 160;
export const NODE_HEIGHT = 44;
const GRID = 20;

export const CATEGORY_LABEL: Record<string, string> = { todo: "To do", in_progress: "In progress", done: "Done" };

export function slugify(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 40);
  return /^[a-z]/.test(slug) ? slug : `s_${slug}`.replace(/_+$/g, "").slice(0, 40);
}

function uniqueKey(definition: WorkflowDefinition, base: string): string {
  let key = base || "status";
  for (let n = 2; definition.statuses.some((s) => s.key === key); n += 1) key = `${base.slice(0, 37)}_${n}`;
  return key;
}

function id(): string {
  return `t_${Math.random().toString(36).slice(2, 10)}`;
}

const snap = (value: number) => Math.max(0, Math.round(value / GRID) * GRID);

/** Add a status below the lowest existing one so it never lands on top of another. */
export function addStatus(definition: WorkflowDefinition, name: string): { definition: WorkflowDefinition; key: string } {
  const key = uniqueKey(definition, slugify(name));
  const bottom = definition.statuses.reduce((max, s) => Math.max(max, s.y ?? 0), -80);
  const status: WorkflowStatus = { key, name: name.trim(), category: "in_progress", x: 60, y: snap(bottom + 80) };
  return { key, definition: { ...definition, statuses: [...definition.statuses, status], initial: definition.initial || key } };
}

export function updateStatus(definition: WorkflowDefinition, key: string, patch: Partial<Pick<WorkflowStatus, "name" | "category">>): WorkflowDefinition {
  return { ...definition, statuses: definition.statuses.map((s) => (s.key === key ? { ...s, ...patch } : s)) };
}

export function moveStatus(definition: WorkflowDefinition, key: string, dx: number, dy: number): WorkflowDefinition {
  return { ...definition, statuses: definition.statuses.map((s) => (s.key === key ? { ...s, x: snap((s.x ?? 0) + dx), y: snap((s.y ?? 0) + dy) } : s)) };
}

/** Remove a status together with every transition that touches it. */
export function removeStatus(definition: WorkflowDefinition, key: string): WorkflowDefinition {
  const statuses = definition.statuses.filter((s) => s.key !== key);
  const transitions = definition.transitions
    .filter((t) => t.to !== key)
    .map((t) => ({ ...t, from: (t.from ?? []).filter((f) => f !== key), wasSpecific: (t.from ?? []).length > 0 }))
    // A transition that started only from the removed status must not silently become "from any status".
    .filter((t) => !(t.wasSpecific && t.from.length === 0))
    .map(({ wasSpecific: _was, ...t }) => t);
  return { statuses, transitions, initial: definition.initial === key ? (statuses[0]?.key ?? "") : definition.initial };
}

export function addTransition(definition: WorkflowDefinition, from: string | null, to: string): { definition: WorkflowDefinition; id: string } {
  const target = definition.statuses.find((s) => s.key === to);
  const transition: WorkflowTransition = { id: id(), name: target?.name ?? "Transition", from: from ? [from] : [], to, conditions: [], validators: [], post_functions: [] };
  return { id: transition.id, definition: { ...definition, transitions: [...definition.transitions, transition] } };
}

export function updateTransition(definition: WorkflowDefinition, transitionId: string, patch: Partial<WorkflowTransition>): WorkflowDefinition {
  return { ...definition, transitions: definition.transitions.map((t) => (t.id === transitionId ? { ...t, ...patch } : t)) };
}

export function removeTransition(definition: WorkflowDefinition, transitionId: string): WorkflowDefinition {
  return { ...definition, transitions: definition.transitions.filter((t) => t.id !== transitionId) };
}

export interface Point {
  x: number;
  y: number;
}

/**
 * Where the arrow between two status boxes starts and ends: on the box borders, along
 * the line between their centres. Opposite directions are nudged apart so both show.
 */
export function edgePoints(from: WorkflowStatus, to: WorkflowStatus): { start: Point; end: Point; label: Point } {
  const centre = (s: WorkflowStatus): Point => ({ x: (s.x ?? 0) + NODE_WIDTH / 2, y: (s.y ?? 0) + NODE_HEIGHT / 2 });
  const a = centre(from);
  const b = centre(to);
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const length = Math.hypot(dx, dy) || 1;
  // Perpendicular offset, signed by direction, separates A→B from B→A.
  const offset = 9;
  const nx = (-dy / length) * offset;
  const ny = (dx / length) * offset;
  const clip = (p: Point, towards: Point): Point => {
    const vx = towards.x - p.x;
    const vy = towards.y - p.y;
    const scale = Math.min((NODE_WIDTH / 2 + 4) / Math.abs(vx || 1e-9), (NODE_HEIGHT / 2 + 4) / Math.abs(vy || 1e-9));
    return { x: p.x + vx * Math.min(scale, 0.5), y: p.y + vy * Math.min(scale, 0.5) };
  };
  const start = clip(a, b);
  const end = clip(b, a);
  return {
    start: { x: start.x + nx, y: start.y + ny },
    end: { x: end.x + nx, y: end.y + ny },
    label: { x: (start.x + end.x) / 2 + nx, y: (start.y + end.y) / 2 + ny },
  };
}

export function describeFrom(definition: WorkflowDefinition, transition: WorkflowTransition): string {
  const from = transition.from ?? [];
  if (from.length === 0) return "Any status";
  return from.map((key) => definition.statuses.find((s) => s.key === key)?.name ?? key).join(", ");
}
