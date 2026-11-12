import { DndContext, DragOverlay, type DragEndEvent, type DragStartEvent } from "@dnd-kit/core";
import { CalendarClock, Flag, Search, Target } from "lucide-react";
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";

import { Button, EmptyState, ErrorState, Input, Skeleton, toast } from "@/components/ui";
import type { Board, BoardCard } from "@/services/api";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";
import { formatDate } from "@/utils/format";

import { NoTransitionError, useBoardView, useDropCard, useSprints } from "../api";
import { AgileFrame, reason } from "../components/AgileFrame";
import { CardTile } from "../components/cards";
import { collision, DraggableCard, DropList, readDrop, useCardSensors } from "../components/dnd";
import { CompleteSprintDialog } from "../components/SprintDialogs";
import { sprintTimeLeft } from "../model";

function BoardBody({ board }: { board: Board }) {
  const view = useBoardView(board);
  const sprints = useSprints(board);
  const drop = useDropCard(board);
  const sensors = useCardSensors();
  const userId = useSession((s) => s.account?.id);
  const [search, setSearch] = useState("");
  const [mine, setMine] = useState(false);
  const [dragging, setDragging] = useState<BoardCard | null>(null);
  const [completing, setCompleting] = useState(false);

  const visible = useMemo(() => {
    const text = search.trim().toLowerCase();
    return (card: BoardCard) =>
      (!mine || card.assignee_id === userId) && (!text || card.summary.toLowerCase().includes(text) || card.key.toLowerCase().includes(text));
  }, [search, mine, userId]);

  if (view.isPending) {
    return <div className="flex gap-3 px-6" aria-busy>{[0, 1, 2].map((i) => <Skeleton key={i} className="h-72 w-72" />)}</div>;
  }
  if (view.isError) return <ErrorState className="py-16" error={view.error} onRetry={() => void view.refetch()} />;

  const { columns, sprint, unmapped, truncated } = view.data;
  const backlogLink = `/projects/${board.project_key}/backlog?board=${board.id}`;
  if (board.kind === "scrum" && !sprint) {
    return (
      <EmptyState
        className="py-20"
        icon={Flag}
        title="No sprint is running"
        description="A Scrum board shows the issues of the running sprint. Plan a sprint in the backlog and start it."
        action={<Button asChild variant="primary"><Link to={backlogLink}>Go to backlog</Link></Button>}
      />
    );
  }

  const canDrag = board.permissions.transition_issues;
  const all = [...columns.flatMap((c) => c.issues), ...unmapped];
  const timeLeft = sprint ? sprintTimeLeft(sprint) : null;

  const onDragStart = (event: DragStartEvent) => setDragging((event.active.data.current?.card as BoardCard | undefined) ?? null);
  const onDragEnd = (event: DragEndEvent) => {
    setDragging(null);
    const dropped = readDrop(event);
    if (!dropped) return;
    const { card, target } = dropped;
    const column = Number(target.list);
    const from = columns.findIndex((c) => c.issues.some((i) => i.id === card.id));
    if (from === column && !target.beforeId) return;
    drop.mutate(
      { card, column, beforeId: target.beforeId },
      {
        onError: (error) =>
          error instanceof NoTransitionError
            ? toast.error(`${card.key} can't move to ${columns[column]?.name}`, "Its workflow has no step from its current status into that column.")
            : toast.error(`Couldn't move ${card.key}`, reason(error)),
      },
    );
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-6 pb-3">
        <div className="relative w-52">
          <Search className="pointer-events-none absolute left-2.5 top-2 h-4 w-4 text-ink-subtle" aria-hidden />
          <Input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search this board" aria-label="Search this board" className="h-8 pl-8 text-sm" />
        </div>
        <Button size="sm" variant={mine ? "primary" : "secondary"} aria-pressed={mine} onClick={() => setMine((v) => !v)}>Only my issues</Button>
        {sprint && (
          <div className="ml-auto flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-ink-muted">
            <span className="font-medium text-ink">{sprint.name}</span>
            {sprint.goal && <span className="inline-flex max-w-xs items-center gap-1.5 truncate" title={sprint.goal}><Target className="h-3.5 w-3.5 shrink-0" aria-hidden />{sprint.goal}</span>}
            {sprint.end_date && (
              <span className="inline-flex items-center gap-1.5" title={`Ends ${formatDate(sprint.end_date)}`}>
                <CalendarClock className="h-3.5 w-3.5" aria-hidden />{timeLeft}
              </span>
            )}
            {board.permissions.manage_sprints && <Button size="sm" onClick={() => setCompleting(true)}>Complete sprint</Button>}
          </div>
        )}
      </div>
      {(unmapped.length > 0 || truncated) && (
        <p className="mx-6 mb-3 rounded border border-warning/30 bg-warning-soft px-3 py-2 text-sm text-warning" role="status">
          {unmapped.length > 0 && `${unmapped.length} ${unmapped.length === 1 ? "issue is" : "issues are"} in a status that no column shows (${[...new Set(unmapped.map((c) => c.status))].join(", ")}). `}
          {unmapped.length > 0 && board.permissions.manage_board && "Add the status to a column in Board settings. "}
          {truncated && "This project has more open issues than a board can show; the oldest are listed first."}
        </p>
      )}
      <DndContext sensors={sensors} collisionDetection={collision} onDragStart={onDragStart} onDragEnd={onDragEnd} onDragCancel={() => setDragging(null)}>
        <div className="flex min-h-0 flex-1 gap-3 overflow-x-auto px-6 pb-6">
          {columns.map((column, index) => {
            const cards = column.issues.filter(visible);
            const over = column.wip_limit != null && column.issues.length > column.wip_limit;
            return (
              <section key={column.id} aria-label={column.name} className={cn("flex w-72 shrink-0 flex-col rounded-md bg-sunken", over && "bg-danger-soft")}>
                <h2 className="flex items-center gap-2 px-3 py-2.5 text-xs font-semibold uppercase tracking-wide text-ink-muted">
                  <span className="truncate">{column.name}</span>
                  <span className="font-normal">{cards.length === column.issues.length ? column.issues.length : `${cards.length} of ${column.issues.length}`}</span>
                  {column.wip_limit != null && (
                    <span className={cn("ml-auto rounded-sm px-1.5 font-medium normal-case tracking-normal", over ? "bg-danger text-white" : "bg-line text-ink-muted")} title="Issue limit for this column">
                      Max {column.wip_limit}
                    </span>
                  )}
                </h2>
                <DropList list={String(index)} className="min-h-24 flex-1 space-y-1.5 overflow-y-auto rounded-b-md px-1.5 pb-1.5">
                  {cards.map((card) => (
                    <DraggableCard key={card.id} card={card} list={String(index)} disabled={!canDrag}>
                      <CardTile card={card} />
                    </DraggableCard>
                  ))}
                </DropList>
              </section>
            );
          })}
        </div>
        <DragOverlay>{dragging && <div className="w-[17.25rem]"><CardTile card={dragging} overlay /></div>}</DragOverlay>
      </DndContext>
      {sprint && (
        <CompleteSprintDialog
          board={board}
          sprint={sprint}
          issues={all}
          planned={(sprints.data ?? []).filter((s) => s.state === "future")}
          open={completing}
          onOpenChange={setCompleting}
        />
      )}
    </div>
  );
}

export default function BoardPage() {
  return <AgileFrame title="Board">{(board) => <BoardBody board={board} />}</AgileFrame>;
}
