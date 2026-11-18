import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";

import { issueKeys } from "@/modules/issues";
import {
  api,
  requestWithMeta,
  type BacklogView,
  type Board,
  type BoardCard,
  type BoardColumn,
  type BoardList,
  type BoardView,
  type QuickFilter,
  type Sprint,
  type SprintList,
  type SprintReport,
  type TransitionList,
  type VelocityReport,
} from "@/services/api";
import { useSession } from "@/state/session";

import { applyCardMove, applyMove, changesColumn, moveRequest, transitionInto, type Move } from "./model";

const BOARDS = "/api/v1/boards";
const SPRINTS = "/api/v1/sprints";

const useOrgId = () => useSession((s) => s.organization?.id);

// Board data is made of issues, so it lives under the project's issue-list key: every
// issue create or edit in the project already invalidates that prefix, which refreshes
// boards and backlogs without this module having to know about those mutations.
const keys = {
  root: (orgId: string | undefined, projectKey: string) => [...issueKeys.lists(orgId, projectKey), "agile"] as const,
  boards: (orgId: string | undefined, projectKey: string) => ["org", orgId, "boards", projectKey] as const,
  view: (orgId: string | undefined, projectKey: string, boardId: string) => [...keys.root(orgId, projectKey), boardId, "view"] as const,
  backlog: (orgId: string | undefined, projectKey: string, boardId: string) => [...keys.root(orgId, projectKey), boardId, "backlog"] as const,
  sprints: (orgId: string | undefined, projectKey: string, boardId: string) => [...keys.root(orgId, projectKey), boardId, "sprints"] as const,
  velocity: (orgId: string | undefined, projectKey: string, boardId: string) => [...keys.root(orgId, projectKey), boardId, "velocity"] as const,
  report: (orgId: string | undefined, projectKey: string, sprintId: string) => [...keys.root(orgId, projectKey), "report", sprintId] as const,
};

const refresh = (qc: QueryClient, orgId: string | undefined, projectKey: string) =>
  qc.invalidateQueries({ queryKey: keys.root(orgId, projectKey) });

export function useBoards(projectKey: string) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.boards(orgId, projectKey),
    queryFn: () => api.get<BoardList>(BOARDS, { query: { project_key: projectKey } }),
    select: (d) => d.items,
    enabled: Boolean(orgId && projectKey),
  });
}

export function useBoardView(board: Board | undefined) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.view(orgId, board?.project_key ?? "", board?.id ?? ""),
    queryFn: () => api.get<BoardView>(`${BOARDS}/${board!.id}/view`),
    enabled: Boolean(orgId && board),
  });
}

export function useBacklog(board: Board | undefined) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.backlog(orgId, board?.project_key ?? "", board?.id ?? ""),
    queryFn: () => api.get<BacklogView>(`${BOARDS}/${board!.id}/backlog`),
    enabled: Boolean(orgId && board),
  });
}

export function useSprints(board: Board | undefined) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.sprints(orgId, board?.project_key ?? "", board?.id ?? ""),
    queryFn: () => api.get<SprintList>(`${BOARDS}/${board!.id}/sprints`),
    select: (d) => d.items,
    enabled: Boolean(orgId && board),
  });
}

export function useVelocity(board: Board | undefined) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.velocity(orgId, board?.project_key ?? "", board?.id ?? ""),
    queryFn: () => api.get<VelocityReport>(`${BOARDS}/${board!.id}/velocity`),
    select: (d) => d.sprints,
    enabled: Boolean(orgId && board),
  });
}

export function useSprintReport(projectKey: string, sprintId: string | undefined) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.report(orgId, projectKey, sprintId ?? ""),
    queryFn: () => api.get<SprintReport>(`${SPRINTS}/${sprintId}/report`),
    enabled: Boolean(orgId && sprintId),
  });
}

export function useBoardMutations(projectKey: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => {
    void qc.invalidateQueries({ queryKey: keys.boards(orgId, projectKey) });
    void refresh(qc, orgId, projectKey);
  };
  return {
    create: useMutation({
      mutationFn: (input: { name: string; kind: string }) => api.post<Board>(BOARDS, { project_key: projectKey, ...input }),
      onSuccess: done,
    }),
    update: useMutation({
      mutationFn: async ({ board, patch }: { board: Board; patch: { name?: string; columns?: BoardColumn[]; quick_filters?: QuickFilter[] } }) =>
        (await requestWithMeta<Board>("PATCH", `${BOARDS}/${board.id}`, { body: patch, ifMatch: board.version })).data,
      // Also on failure: a conflict means someone else changed the board.
      onSettled: done,
    }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${BOARDS}/${id}`), onSuccess: done }),
  };
}

/** Reorder issues or move them between sprints and the backlog, updating the view at once. */
export function useMoveIssues(board: Board) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const queryKey = keys.backlog(orgId, board.project_key, board.id);
  return useMutation({
    mutationFn: ({ move, base }: { move: Move; base: BacklogView }) => api.post(`${BOARDS}/${board.id}/move`, moveRequest(base, move)),
    onMutate: async ({ move }) => {
      await qc.cancelQueries({ queryKey });
      const previous = qc.getQueryData<BacklogView>(queryKey);
      if (previous) qc.setQueryData(queryKey, applyMove(previous, move));
      return { previous };
    },
    onError: (_error, _input, context) => {
      if (context?.previous) qc.setQueryData(queryKey, context.previous);
    },
    onSettled: () => refresh(qc, orgId, board.project_key),
  });
}

export interface CardDrop {
  card: BoardCard;
  /** Index of the column the card was dropped in. */
  column: number;
  /** The card it was dropped onto, if any. */
  beforeId?: string;
}

export class NoTransitionError extends Error {}

/**
 * Dropping a card on the board. Changing column is a status change, so it goes through
 * the issue's workflow: the step leading into the column is looked up and requested,
 * and a refusal (with the workflow's explanation) puts the card back.
 */
export function useDropCard(board: Board) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const queryKey = keys.view(orgId, board.project_key, board.id);
  return useMutation({
    mutationFn: async ({ card, column, beforeId }: CardDrop) => {
      const columns = qc.getQueryData<BoardView>(queryKey)?.board.columns ?? board.columns;
      // Decided from the card as it was picked up: the cached board already shows the
      // optimistic result, so it cannot say where the card came from.
      if (changesColumn(columns, card, column)) {
        const offered = await api.get<TransitionList>(`/api/v1/issues/${card.key}/transitions`);
        const step = transitionInto(columns, column, offered.items);
        if (!step) throw new NoTransitionError();
        await requestWithMeta("PATCH", `/api/v1/issues/${card.key}`, { body: { status: step.to.key }, ifMatch: card.version });
      }
      if (beforeId && board.permissions.move_issues) {
        await api.post(`${BOARDS}/${board.id}/move`, { issue_ids: [card.id], before_id: beforeId });
      }
    },
    onMutate: async ({ card, column, beforeId }) => {
      await qc.cancelQueries({ queryKey });
      const previous = qc.getQueryData<BoardView>(queryKey);
      if (previous) qc.setQueryData(queryKey, applyCardMove(previous, card.id, column, beforeId));
      return { previous };
    },
    onError: (_error, _input, context) => {
      if (context?.previous) qc.setQueryData(queryKey, context.previous);
    },
    onSettled: (_data, _error, { card }) => {
      void refresh(qc, orgId, board.project_key);
      void qc.invalidateQueries({ queryKey: issueKeys.one(orgId, card.key) });
    },
  });
}

export interface SprintInput {
  name?: string;
  goal?: string;
  start_date?: string | null;
  end_date?: string | null;
}

export function useSprintMutations(board: Board) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const done = () => refresh(qc, orgId, board.project_key);
  // Dates are optional on create: the API takes "absent", not null.
  const compact = (input: SprintInput) => Object.fromEntries(Object.entries(input).filter(([, v]) => v !== null && v !== ""));
  return {
    create: useMutation({ mutationFn: (input: SprintInput) => api.post<Sprint>(`${BOARDS}/${board.id}/sprints`, compact(input)), onSuccess: done }),
    update: useMutation({
      mutationFn: async ({ sprint, patch }: { sprint: Sprint; patch: SprintInput }) =>
        (await requestWithMeta<Sprint>("PATCH", `${SPRINTS}/${sprint.id}`, { body: patch, ifMatch: sprint.version })).data,
      onSettled: done,
    }),
    remove: useMutation({ mutationFn: (id: string) => api.delete(`${SPRINTS}/${id}`), onSuccess: done }),
    start: useMutation({
      mutationFn: ({ id, ...body }: { id: string; name: string; goal: string; end_date: string }) => api.post<Sprint>(`${SPRINTS}/${id}/start`, body),
      onSuccess: done,
    }),
    complete: useMutation({
      mutationFn: ({ id, moveTo }: { id: string; moveTo: string | null }) =>
        api.post<Sprint>(`${SPRINTS}/${id}/complete`, moveTo ? { move_incomplete_to: moveTo } : {}),
      onSuccess: done,
    }),
  };
}
