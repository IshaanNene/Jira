import { DndContext, DragOverlay, type DragEndEvent, type DragStartEvent } from "@dnd-kit/core";
import { Layers, MoreHorizontal, Plus } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";

import {
  Badge,
  Button,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  ErrorState,
  Skeleton,
  toast,
} from "@/components/ui";
import { useProjectStatuses } from "@/modules/issues";
import { MqlFilterButton, MqlFilterPanel, useMqlFilter } from "@/modules/search";
import type { BacklogView, Board, BoardCard, Sprint } from "@/services/api";
import { useUi } from "@/state/ui";
import { formatDate, titleCase } from "@/utils/format";

import { useBacklog, useMoveIssues, useSprintMutations } from "../api";
import { AgileFrame, reason } from "../components/AgileFrame";
import { CardRow } from "../components/cards";
import { collision, DraggableCard, DropList, readDrop, useCardSensors } from "../components/dnd";
import { effectiveEpic, EpicFilter } from "../components/EpicFilter";
import { EpicPanel } from "../components/EpicPanel";
import { QuickFilterBar, useQuickFilters } from "../components/QuickFilters";
import { CompleteSprintDialog, SprintDialog, StartSprintDialog } from "../components/SprintDialogs";
import { ALL_EPICS, BACKLOG, formatPoints, inEpic, totalPoints, type Move } from "../model";

type SprintAction = { kind: "create" } | { kind: "edit" | "start" | "complete"; sprint: Sprint };

function Summary({ issues, shown }: { issues: BoardCard[]; shown: BoardCard[] }) {
  const count = shown.length === issues.length ? `${issues.length}` : `${shown.length} of ${issues.length}`;
  return (
    <span className="text-sm text-ink-subtle">
      {count} {issues.length === 1 ? "issue" : "issues"} · {formatPoints(totalPoints(issues))} pts
    </span>
  );
}

function BacklogBody({ board }: { board: Board }) {
  const backlog = useBacklog(board);
  const statuses = useProjectStatuses(board.project_id);
  const move = useMoveIssues(board);
  const { remove } = useSprintMutations(board);
  const sensors = useCardSensors();
  const navigate = useNavigate();
  const openCreateIssue = useUi((s) => s.openCreateIssue);
  const [dragging, setDragging] = useState<BoardCard | null>(null);
  const [action, setAction] = useState<SprintAction | null>(null);
  const quick = useQuickFilters(board);
  const mql = useMqlFilter(board.project_id, quick.queries);
  const [mqlOpen, setMqlOpen] = useState(Boolean(mql.applied));
  const [epicChoice, setEpic] = useState(ALL_EPICS);
  const [epicsOpen, setEpicsOpen] = useState(false);

  if (backlog.isPending) return <div className="space-y-3 px-6" aria-busy><Skeleton className="h-32" /><Skeleton className="h-64" /></div>;
  if (backlog.isError) return <ErrorState className="py-16" error={backlog.error} onRetry={() => void backlog.refetch()} />;

  const data: BacklogView = backlog.data;
  const { permissions } = board;
  const scrum = board.kind === "scrum";
  const running = data.sprints.find((g) => g.sprint.state === "active");
  const firstPlanned = data.sprints.find((g) => g.sprint.state === "future");
  const statusName = (key: string) => statuses.data?.find((s) => s.key === key)?.name ?? titleCase(key);
  const perform = (m: Move, label: string) =>
    move.mutate({ move: m, base: data }, { onError: (error) => toast.error(`Couldn't move ${label}`, reason(error)) });

  const onDragStart = (event: DragStartEvent) => setDragging((event.active.data.current?.card as BoardCard | undefined) ?? null);
  const onDragEnd = (event: DragEndEvent) => {
    setDragging(null);
    const dropped = readDrop(event);
    if (dropped) perform({ issueIds: [dropped.card.id], list: dropped.target.list, beforeId: dropped.target.beforeId }, dropped.card.key);
  };

  const menu = (card: BoardCard, list: string) => {
    if (!permissions.move_issues) return undefined;
    const to = (target: string, beforeId?: string) => perform({ issueIds: [card.id], list: target, beforeId }, card.key);
    const top = data.backlog[0];
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon" className="h-7 w-7" aria-label={`Move ${card.key}`}><MoreHorizontal className="h-4 w-4" /></Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuLabel>Move to</DropdownMenuLabel>
          {data.sprints.filter((g) => g.sprint.id !== list).map((g) => (
            <DropdownMenuItem key={g.sprint.id} onSelect={() => to(g.sprint.id)}>{g.sprint.name}</DropdownMenuItem>
          ))}
          {data.sprints.some((g) => g.sprint.id !== list) && <DropdownMenuSeparator />}
          <DropdownMenuItem onSelect={() => to(BACKLOG, top && top.id !== card.id ? top.id : undefined)}>Top of backlog</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => to(BACKLOG)}>Bottom of backlog</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    );
  };

  // While a filter hides issues, ranking by drag would place issues among ones nobody can
  // see, so it pauses; the row menus still move issues between sprints and the backlog.
  const epic = effectiveEpic(data.epics, epicChoice);
  const filtering = mql.matches !== null || epic !== ALL_EPICS;
  const shown = (issues: BoardCard[]) => issues.filter((i) => inEpic(i, epic) && (!mql.matches || mql.matches.has(i.id)));
  const rows = (issues: BoardCard[], list: string, empty: string) => (
    <DropList list={list} className="min-h-12 overflow-hidden rounded border border-line bg-surface">
      {issues.length === 0 && <p className="px-3 py-3 text-sm text-ink-subtle">{empty}</p>}
      {issues.length > 0 && shown(issues).length === 0 && <p className="px-3 py-3 text-sm text-ink-subtle">No issues here match the filter.</p>}
      {shown(issues).map((card) => (
        <DraggableCard key={card.id} card={card} list={list} disabled={!permissions.move_issues || filtering}>
          <CardRow card={card} statusName={statusName(card.status)} menu={menu(card, list)} />
        </DraggableCard>
      ))}
    </DropList>
  );

  const startBlocked = (sprint: Sprint, issues: BoardCard[]) =>
    running ? `${running.sprint.name} is still running` : sprint.id !== firstPlanned?.sprint.id ? "Start the sprints above first, or reorder by deleting them" : issues.length === 0 ? "Add at least one issue first" : null;

  return (
    <div className="h-full overflow-y-auto px-6 pb-8">
      <div className="mb-4 space-y-3">
        <div className="flex flex-wrap items-center gap-3">
          <Button variant={epicsOpen ? "secondary" : "ghost"} size="sm" aria-pressed={epicsOpen} onClick={() => setEpicsOpen((o) => !o)}>
            <Layers className="h-3.5 w-3.5" /> Epics
          </Button>
          {!epicsOpen && <EpicFilter epics={data.epics} value={epic} onChange={setEpic} />}
          <QuickFilterBar board={board} quick={quick} mql={mql} />
          <MqlFilterButton filter={mql} open={mqlOpen} onOpenChange={setMqlOpen} />
          {filtering && permissions.move_issues && <span className="text-sm text-ink-subtle">Clear the filters to rank issues by dragging.</span>}
        </div>
        {mqlOpen && <MqlFilterPanel filter={mql} label="backlog" onClose={() => setMqlOpen(false)} />}
      </div>
      <div className="flex flex-col gap-4 md:flex-row md:items-start">
      {epicsOpen && <EpicPanel epics={data.epics} value={epic} onChange={setEpic} onClose={() => setEpicsOpen(false)} />}
      <DndContext sensors={sensors} collisionDetection={collision} onDragStart={onDragStart} onDragEnd={onDragEnd} onDragCancel={() => setDragging(null)}>
        <div className="min-w-0 flex-1 space-y-6">
          {data.sprints.map(({ sprint, issues }) => {
            const blocked = sprint.state === "future" ? startBlocked(sprint, issues) : null;
            return (
              <section key={sprint.id} aria-label={sprint.name}>
                <header className="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1">
                  <h2 className="text-base font-semibold text-ink">{sprint.name}</h2>
                  {sprint.state === "active" && <Badge tone="accent">Active</Badge>}
                  {sprint.start_date && sprint.end_date && <span className="text-sm text-ink-subtle">{formatDate(sprint.start_date)} – {formatDate(sprint.end_date)}</span>}
                  <Summary issues={issues} shown={shown(issues)} />
                  {permissions.manage_sprints && (
                    <span className="ml-auto flex items-center gap-1">
                      {sprint.state === "active" ? (
                        <Button size="sm" onClick={() => setAction({ kind: "complete", sprint })}>Complete sprint</Button>
                      ) : (
                        <Button size="sm" variant={blocked ? "secondary" : "primary"} disabled={Boolean(blocked)} title={blocked ?? undefined} onClick={() => setAction({ kind: "start", sprint })}>
                          Start sprint
                        </Button>
                      )}
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button variant="ghost" size="icon" aria-label={`Actions for ${sprint.name}`}><MoreHorizontal className="h-4 w-4" /></Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem onSelect={() => setAction({ kind: "edit", sprint })}>Edit sprint</DropdownMenuItem>
                          {sprint.state === "future" && (
                            <DropdownMenuItem
                              tone="danger"
                              onSelect={() =>
                                remove.mutate(sprint.id, {
                                  onSuccess: () => toast.success(`Deleted ${sprint.name}`, issues.length > 0 ? "Its issues are back in the backlog." : undefined),
                                  onError: (error) => toast.error("Couldn't delete the sprint", reason(error)),
                                })
                              }
                            >
                              Delete sprint
                            </DropdownMenuItem>
                          )}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </span>
                  )}
                </header>
                {sprint.goal && <p className="mb-2 text-sm text-ink-muted">{sprint.goal}</p>}
                {rows(issues, sprint.id, permissions.move_issues ? "Plan this sprint by dragging issues here from the backlog." : "No issues are planned for this sprint.")}
              </section>
            );
          })}

          <section aria-label="Backlog">
            <header className="mb-2 flex flex-wrap items-center gap-3">
              <h2 className="text-base font-semibold text-ink">Backlog</h2>
              <Summary issues={data.backlog} shown={shown(data.backlog)} />
              {scrum && permissions.manage_sprints && (
                <Button size="sm" className="ml-auto" onClick={() => setAction({ kind: "create" })}><Plus className="h-3.5 w-3.5" /> Create sprint</Button>
              )}
            </header>
            {rows(data.backlog, BACKLOG, "The backlog is empty. Unfinished issues that are not in a sprint appear here.")}
            <Button variant="ghost" size="sm" className="mt-2" onClick={() => openCreateIssue({ projectKey: board.project_key })}><Plus className="h-3.5 w-3.5" /> Create issue</Button>
            {data.truncated && <p className="mt-2 text-sm text-warning" role="status">This project has more open issues than can be shown here; the oldest are listed.</p>}
          </section>
        </div>
        <DragOverlay>{dragging && <CardRow card={dragging} statusName={statusName(dragging.status)} overlay />}</DragOverlay>
      </DndContext>
      </div>

      <SprintDialog board={board} sprint={action?.kind === "edit" ? action.sprint : undefined} open={action?.kind === "create" || action?.kind === "edit"} onOpenChange={(open) => !open && setAction(null)} />
      {action?.kind === "start" && (
        <StartSprintDialog board={board} sprint={action.sprint} issues={data.sprints.find((g) => g.sprint.id === action.sprint.id)?.issues ?? []} open onOpenChange={(open) => !open && setAction(null)} />
      )}
      {action?.kind === "complete" && (
        <CompleteSprintDialog
          board={board}
          sprint={action.sprint}
          issues={data.sprints.find((g) => g.sprint.id === action.sprint.id)?.issues ?? []}
          planned={data.sprints.filter((g) => g.sprint.state === "future").map((g) => g.sprint)}
          open
          onOpenChange={(open) => !open && setAction(null)}
          onCompleted={(closed) => navigate(`/projects/${board.project_key}/reports?board=${board.id}&sprint=${closed.id}`)}
        />
      )}
    </div>
  );
}

export default function BacklogPage() {
  return <AgileFrame title="Backlog">{(board) => <BacklogBody board={board} />}</AgileFrame>;
}
