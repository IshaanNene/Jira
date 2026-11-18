import { ArrowDown, ArrowUp, KanbanSquare, Plus, Settings2, Trash2, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useParams, useSearchParams } from "react-router-dom";

import {
  Button,
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
import { useProjectStatuses } from "@/modules/issues";
import { roleAtLeast, useProject } from "@/modules/projects";
import { mqlProblem } from "@/modules/search";
import { ApiError, errorMessage, type Board, type BoardColumn, type QuickFilter } from "@/services/api";

import { useBoardMutations, useBoards } from "../api";

/** The most specific explanation the server gave. */
export function reason(error: unknown): string {
  return error instanceof ApiError && error.fieldErrors[0] ? error.fieldErrors[0].message : errorMessage(error);
}

const KINDS = [
  { value: "scrum", label: "Scrum", description: "Plan work into sprints from a backlog" },
  { value: "kanban", label: "Kanban", description: "A continuous flow of work, no sprints" },
];

function CreateBoardDialog({ projectKey, open, onOpenChange, onCreated }: { projectKey: string; open: boolean; onOpenChange: (open: boolean) => void; onCreated: (board: Board) => void }) {
  const { create } = useBoardMutations(projectKey);
  const [name, setName] = useState("");
  const [kind, setKind] = useState("scrum");
  const [error, setError] = useState<string>();

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setError(undefined);
    create.mutate(
      { name, kind },
      {
        onSuccess: (board) => {
          toast.success(`Created ${board.name}`);
          setName("");
          onOpenChange(false);
          onCreated(board);
        },
        onError: (e) => setError(reason(e)),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Create board" description={`A new board for ${projectKey}. Columns start as To Do, In Progress and Done.`}>
        <form onSubmit={submit} className="space-y-4">
          <Field label="Name" error={error}>
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={`${projectKey} board`} maxLength={80} autoFocus required />
          </Field>
          <Field label="Type">
            <Select value={kind} onValueChange={setKind} options={KINDS} />
          </Field>
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" loading={create.isPending}>Create board</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

const CATCH = [
  { value: "none", label: "Listed statuses only" },
  { value: "todo", label: "+ any other To Do status" },
  { value: "in_progress", label: "+ any other In Progress status" },
  { value: "done", label: "+ any other Done status" },
];

function BoardSettingsDialog({ board, open, onOpenChange }: { board: Board; open: boolean; onOpenChange: (open: boolean) => void }) {
  const { update, remove } = useBoardMutations(board.project_key);
  const statuses = useProjectStatuses(board.project_id);
  const [name, setName] = useState(board.name);
  const [columns, setColumns] = useState<BoardColumn[]>(board.columns);
  const [quickFilters, setQuickFilters] = useState<QuickFilter[]>(board.quick_filters);
  const [checking, setChecking] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string>();

  // Start from the saved board each time the dialog opens.
  useEffect(() => {
    if (open) {
      setName(board.name);
      setColumns(board.columns);
      setQuickFilters(board.quick_filters);
      setConfirmDelete(false);
      setError(undefined);
    }
  }, [open, board]);

  const options = (statuses.data ?? []).map((s) => ({ value: s.key, label: s.name }));
  const patch = (index: number, change: Partial<BoardColumn>) => setColumns((cols) => cols.map((c, i) => (i === index ? { ...c, ...change } : c)));
  const swap = (index: number, other: number) =>
    setColumns((cols) => {
      const next = [...cols];
      [next[index], next[other]] = [next[other]!, next[index]!];
      return next;
    });
  const add = () => setColumns((cols) => [...cols, { id: `col-${Date.now().toString(36)}`, name: "", statuses: [], category: null, wip_limit: null }]);
  const patchQuick = (index: number, change: Partial<QuickFilter>) => setQuickFilters((all) => all.map((f, i) => (i === index ? { ...f, ...change } : f)));
  const addQuick = () => setQuickFilters((all) => [...all, { id: `qf-${Date.now().toString(36)}`, name: "", query: "" }]);

  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(undefined);
    // New and changed queries must run before they are saved; unchanged ones already did.
    const saved = new Map(board.quick_filters.map((f) => [f.id, f.query.trim()]));
    const changed = quickFilters.filter((f) => f.query.trim() && saved.get(f.id) !== f.query.trim());
    if (changed.length > 0) {
      setChecking(true);
      try {
        for (const filter of changed) {
          const problem = await mqlProblem(board.project_id, filter.query);
          if (problem) {
            setError(`The query of "${filter.name.trim() || "a quick filter"}" can't run: ${problem}`);
            return;
          }
        }
      } catch (e) {
        setError(`Couldn't check the quick filters' queries: ${reason(e)}`);
        return;
      } finally {
        setChecking(false);
      }
    }
    update.mutate(
      { board, patch: { name, columns, quick_filters: quickFilters } },
      {
        onSuccess: () => {
          toast.success("Board saved");
          onOpenChange(false);
        },
        onError: (e) =>
          setError(e instanceof ApiError && e.isConflict ? "Someone else changed this board. Close this dialog and reopen it to see their changes." : reason(e)),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Board settings" description="Each column shows the statuses listed for it. Dragging a card into a column moves the issue through its workflow." className="top-[8%] max-w-3xl">
        <form onSubmit={(e) => void save(e)} className="space-y-4">
          <Field label="Board name">
            <Input value={name} onChange={(e) => setName(e.target.value)} maxLength={80} required />
          </Field>
          <div>
            <div className="mb-1.5 text-sm font-medium text-ink">Columns</div>
            <ul className="max-h-[45vh] space-y-2 overflow-y-auto pr-1">
              {columns.map((column, index) => (
                <li key={column.id} className="grid grid-cols-[1fr_1.2fr_1.2fr_5rem_auto] items-center gap-2 rounded border border-line bg-sunken/50 p-2">
                  <Input value={column.name} onChange={(e) => patch(index, { name: e.target.value })} placeholder="Column name" aria-label={`Name of column ${index + 1}`} maxLength={40} className="h-7 text-sm" required />
                  <MultiSelect values={column.statuses ?? []} onChange={(values) => patch(index, { statuses: values })} options={options} placeholder="No listed statuses" aria-label={`Statuses in ${column.name || `column ${index + 1}`}`} />
                  <Select size="sm" value={column.category ?? "none"} onValueChange={(v) => patch(index, { category: v === "none" ? null : v })} options={CATCH} aria-label={`Other statuses shown in ${column.name || `column ${index + 1}`}`} />
                  <Input type="number" min={1} max={999} value={column.wip_limit ?? ""} onChange={(e) => patch(index, { wip_limit: e.target.value ? Number(e.target.value) : null })} placeholder="Limit" aria-label={`Issue limit for ${column.name || `column ${index + 1}`}`} className="h-7 px-2 text-sm" />
                  <span className="flex">
                    <Button variant="ghost" size="icon" className="h-7 w-7" disabled={index === 0} onClick={() => swap(index, index - 1)} aria-label="Move column left"><ArrowUp className="h-3.5 w-3.5" /></Button>
                    <Button variant="ghost" size="icon" className="h-7 w-7" disabled={index === columns.length - 1} onClick={() => swap(index, index + 1)} aria-label="Move column right"><ArrowDown className="h-3.5 w-3.5" /></Button>
                    <Button variant="ghost" size="icon" className="h-7 w-7" disabled={columns.length === 1} onClick={() => setColumns((cols) => cols.filter((_, i) => i !== index))} aria-label={`Remove column ${column.name}`}><Trash2 className="h-3.5 w-3.5" /></Button>
                  </span>
                </li>
              ))}
            </ul>
            <Button size="sm" variant="ghost" className="mt-2" onClick={add} disabled={columns.length >= 12}><Plus className="h-3.5 w-3.5" /> Add column</Button>
          </div>
          <div>
            <div className="mb-1.5 text-sm font-medium text-ink">Quick filters</div>
            <p className="mb-2 text-xs text-ink-subtle">Buttons above the board and backlog that show only the issues an MQL condition matches. Several switched on must all match.</p>
            {quickFilters.length > 0 && (
              <ul className="max-h-[30vh] space-y-2 overflow-y-auto pr-1">
                {quickFilters.map((filter, index) => (
                  <li key={filter.id} className="grid grid-cols-[1fr_2fr_auto] items-center gap-2 rounded border border-line bg-sunken/50 p-2">
                    <Input value={filter.name} onChange={(e) => patchQuick(index, { name: e.target.value })} placeholder="Button label" aria-label={`Name of quick filter ${index + 1}`} maxLength={40} className="h-7 text-sm" required />
                    <Input value={filter.query} onChange={(e) => patchQuick(index, { query: e.target.value })} placeholder="type = bug AND priority >= high" aria-label={`Query of ${filter.name || `quick filter ${index + 1}`}`} maxLength={1000} className="h-7 font-mono text-xs" required />
                    <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => setQuickFilters((all) => all.filter((_, i) => i !== index))} aria-label={`Remove quick filter ${filter.name || index + 1}`}><X className="h-3.5 w-3.5" /></Button>
                  </li>
                ))}
              </ul>
            )}
            <Button size="sm" variant="ghost" className="mt-2" onClick={addQuick} disabled={quickFilters.length >= 20}><Plus className="h-3.5 w-3.5" /> Add quick filter</Button>
          </div>
          {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          <div className="flex items-center justify-between gap-2 pt-1">
            {confirmDelete ? (
              <span className="flex items-center gap-2 text-sm text-ink-muted">
                Delete this board? Issues are not deleted.
                <Button
                  size="sm"
                  variant="danger"
                  loading={remove.isPending}
                  onClick={() =>
                    remove.mutate(board.id, {
                      onSuccess: () => {
                        toast.success(`Deleted ${board.name}`);
                        onOpenChange(false);
                      },
                      onError: (e) => setError(reason(e)),
                    })
                  }
                >
                  Delete
                </Button>
                <Button size="sm" onClick={() => setConfirmDelete(false)}>Keep</Button>
              </span>
            ) : (
              <Button variant="ghost" onClick={() => setConfirmDelete(true)}><Trash2 className="h-3.5 w-3.5" /> Delete board</Button>
            )}
            <span className="flex gap-2">
              <Button onClick={() => onOpenChange(false)}>Cancel</Button>
              <Button type="submit" variant="primary" loading={checking || update.isPending}>Save</Button>
            </span>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Shared frame of the board, backlog and reports pages: resolves which board of the
 * project is selected (`?board=`, else the first), and offers switching, creating and
 * configuring boards.
 */
export function AgileFrame({ title, actions, children }: { title: string; actions?: (board: Board) => ReactNode; children: (board: Board) => ReactNode }) {
  const { projectKey = "" } = useParams();
  const key = projectKey.toUpperCase();
  const [params, setParams] = useSearchParams();
  const boards = useBoards(key);
  const project = useProject(key);
  const [creating, setCreating] = useState(false);
  const [configuring, setConfiguring] = useState(false);
  const isAdmin = roleAtLeast(project.data?.my_role, "administrator");

  const select = (id: string) => {
    const next = new URLSearchParams(params);
    next.set("board", id);
    next.delete("sprint");
    setParams(next, { replace: true });
  };
  const board = boards.data?.find((b) => b.id === params.get("board")) ?? boards.data?.[0];
  const createDialog = <CreateBoardDialog projectKey={key} open={creating} onOpenChange={setCreating} onCreated={(b) => select(b.id)} />;

  if (boards.isPending) {
    return <div className="space-y-4 p-6" aria-busy><Skeleton className="h-8 w-56" /><Skeleton className="h-64" /></div>;
  }
  if (boards.isError) return <ErrorState className="py-24" error={boards.error} onRetry={() => void boards.refetch()} />;
  if (!board) {
    return (
      <>
        <EmptyState
          className="py-24"
          icon={KanbanSquare}
          title="This project has no board yet"
          description={isAdmin ? "Create a Scrum board to plan sprints from a backlog, or a Kanban board for a continuous flow of work." : "A project administrator can create one."}
          action={isAdmin ? <Button variant="primary" onClick={() => setCreating(true)}><Plus className="h-4 w-4" /> Create board</Button> : undefined}
        />
        {createDialog}
      </>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex flex-wrap items-center justify-between gap-3 px-6 pb-3 pt-6">
        <div className="min-w-0">
          <p className="text-xs text-ink-subtle">{board.name} · {board.kind === "scrum" ? "Scrum" : "Kanban"} board</p>
          <h1 className="text-2xl font-semibold tracking-tight text-ink">{title}</h1>
        </div>
        <div className="flex items-center gap-2">
          {actions?.(board)}
          {(boards.data?.length ?? 0) > 1 && (
            <Select size="sm" className="w-44" aria-label="Board" value={board.id} onValueChange={select} options={boards.data!.map((b) => ({ value: b.id, label: b.name }))} />
          )}
          {board.permissions.manage_board && (
            <>
              <Button size="sm" onClick={() => setConfiguring(true)}><Settings2 className="h-3.5 w-3.5" /> Board settings</Button>
              <Button size="sm" variant="ghost" onClick={() => setCreating(true)}><Plus className="h-3.5 w-3.5" /> New board</Button>
            </>
          )}
        </div>
      </header>
      <div className="min-h-0 flex-1">{children(board)}</div>
      {createDialog}
      <BoardSettingsDialog board={board} open={configuring} onOpenChange={setConfiguring} />
    </div>
  );
}
