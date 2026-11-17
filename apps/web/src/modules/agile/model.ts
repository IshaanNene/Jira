// Pure board logic: where a card belongs, what a drop means, and chart series.
// No React and no I/O, so it is unit-tested directly.
import type { BacklogView, BoardCard, BoardColumn, BoardView, BurndownSample, IssueTransition, Sprint } from "@/services/api";

/** Target of a drop in the planning view: a sprint id, or the backlog. */
export const BACKLOG = "backlog";

export interface Move {
  issueIds: string[];
  /** Sprint id or `BACKLOG`. */
  list: string;
  /** Insert before this issue; omitted = at the end of the list. */
  beforeId?: string;
}

export interface MoveRequest {
  issue_ids: string[];
  sprint_id: string | null;
  before_id?: string;
  after_id?: string;
}

const listOf = (view: BacklogView, list: string): BoardCard[] | undefined =>
  list === BACKLOG ? view.backlog : view.sprints.find((g) => g.sprint.id === list)?.issues;

/** The planning view after a move, for an instant (optimistic) update. */
export function applyMove(view: BacklogView, move: Move): BacklogView {
  const moving = new Set(move.issueIds);
  const everything = [...view.sprints.flatMap((g) => g.issues), ...view.backlog];
  const picked = move.issueIds.map((id) => everything.find((c) => c.id === id)).filter((c): c is BoardCard => Boolean(c));
  const without = (cards: BoardCard[]) => cards.filter((c) => !moving.has(c.id));
  const next: BacklogView = {
    ...view,
    sprints: view.sprints.map((g) => ({ ...g, issues: without(g.issues) })),
    backlog: without(view.backlog),
  };
  const target = listOf(next, move.list);
  if (!target) return view;
  const sprintId = move.list === BACKLOG ? null : move.list;
  const index = move.beforeId ? target.findIndex((c) => c.id === move.beforeId) : -1;
  target.splice(index < 0 ? target.length : index, 0, ...picked.map((c) => ({ ...c, sprint_id: sprintId })));
  return next;
}

/**
 * What to ask the server for. Positions are always expressed relative to an issue that
 * is not being moved; an empty list needs no position (the issues keep their sprint
 * change and go to the bottom of the overall order).
 */
export function moveRequest(view: BacklogView, move: Move): MoveRequest {
  const request: MoveRequest = { issue_ids: move.issueIds, sprint_id: move.list === BACKLOG ? null : move.list };
  const moving = new Set(move.issueIds);
  if (move.beforeId && !moving.has(move.beforeId)) return { ...request, before_id: move.beforeId };
  const rest = (listOf(view, move.list) ?? []).filter((c) => !moving.has(c.id));
  const last = rest.at(-1);
  return last ? { ...request, after_id: last.id } : request;
}

/** Index of the column showing a status: an explicit mapping wins over the category catch-all. */
export function columnIndex(columns: BoardColumn[], status: string, category: string): number {
  const explicit = columns.findIndex((c) => (c.statuses ?? []).includes(status));
  return explicit >= 0 ? explicit : columns.findIndex((c) => c.category === category);
}

/** Whether dropping `card` into column `target` means changing its status. */
export function changesColumn(columns: BoardColumn[], card: Pick<BoardCard, "status" | "status_category">, target: number): boolean {
  return columnIndex(columns, card.status, card.status_category) !== target;
}

/** The workflow step that takes an issue into a column, if the workflow offers one. */
export function transitionInto(columns: BoardColumn[], target: number, transitions: IssueTransition[]): IssueTransition | undefined {
  return transitions.find((t) => columnIndex(columns, t.to.key, t.to.category) === target);
}

/** The board after a card changes column (and optionally position), for an instant update. */
export function applyCardMove(view: BoardView, cardId: string, target: number, beforeId?: string): BoardView {
  const card = view.columns.flatMap((c) => c.issues).find((c) => c.id === cardId);
  if (!card || !view.columns[target]) return view;
  const columns = view.columns.map((c) => ({ ...c, issues: c.issues.filter((i) => i.id !== cardId) }));
  const issues = columns[target]!.issues;
  const index = beforeId ? issues.findIndex((i) => i.id === beforeId) : -1;
  issues.splice(index < 0 ? issues.length : index, 0, card);
  return { ...view, columns };
}

export function totalPoints(cards: { story_points?: number | null }[]): number {
  return cards.reduce((sum, c) => sum + (c.story_points ?? 0), 0);
}

/** 3 → "3", 2.5 → "2.5". */
export function formatPoints(points: number): string {
  return Number.isInteger(points) ? String(points) : points.toFixed(1);
}

const DAY = 24 * 3600 * 1000;

/** "3 days left", "Ends today", "2 days over". */
export function sprintTimeLeft(sprint: Pick<Sprint, "end_date">, now: Date = new Date()): string | null {
  if (!sprint.end_date) return null;
  const end = new Date(`${sprint.end_date}T23:59:59`);
  const days = Math.ceil((end.getTime() - now.getTime()) / DAY) - 1;
  if (days > 0) return `${days} ${days === 1 ? "day" : "days"} left`;
  if (days === 0) return "Ends today";
  return `${-days} ${days === -1 ? "day" : "days"} over`;
}

export type BurndownUnit = "points" | "issues";

export interface BurndownSeries {
  /** [time in ms, remaining] */
  remaining: [number, number][];
  /** Straight line from the starting scope to zero on the planned end date. */
  guideline: [number, number][];
}

export function burndownSeries(samples: BurndownSample[], sprint: Pick<Sprint, "end_date">, unit: BurndownUnit): BurndownSeries {
  const value = (s: BurndownSample) => (unit === "points" ? s.remaining_points : s.remaining_issues);
  const remaining = samples.map((s): [number, number] => [new Date(s.at).getTime(), value(s)]);
  const first = samples[0];
  if (!first) return { remaining, guideline: [] };
  const start = new Date(first.at).getTime();
  const lastSample = remaining.at(-1)![0];
  const planned = sprint.end_date ? new Date(`${sprint.end_date}T23:59:59Z`).getTime() : lastSample;
  const end = Math.max(planned, start + DAY);
  const scope = unit === "points" ? first.scope_points : first.scope_issues;
  return { remaining, guideline: [[start, scope], [end, 0]] };
}

/** Epic filter values besides an epic's key. */
export const ALL_EPICS = "all";
export const NO_EPIC = "none";

/** Whether a card passes the epic filter: every card, cards with no epic, or one epic's. */
export function inEpic(card: BoardCard, epic: string): boolean {
  if (epic === ALL_EPICS) return true;
  if (epic === NO_EPIC) return !card.parent_key;
  return card.parent_key === epic;
}
