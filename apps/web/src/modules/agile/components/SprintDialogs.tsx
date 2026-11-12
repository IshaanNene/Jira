import { useEffect, useState } from "react";

import { Button, Dialog, DialogContent, DialogFooter, Field, Input, Select, Textarea, toast } from "@/components/ui";
import { ApiError, type Board, type BoardCard, type Sprint } from "@/services/api";

import { useSprintMutations } from "../api";
import { formatPoints, totalPoints } from "../model";
import { reason } from "./AgileFrame";

const isoDate = (date: Date) => date.toISOString().slice(0, 10);
const inDays = (days: number) => isoDate(new Date(Date.now() + days * 24 * 3600 * 1000));

interface DialogProps {
  board: Board;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Create a sprint, or edit a planned or running one. */
export function SprintDialog({ board, sprint, open, onOpenChange }: DialogProps & { sprint?: Sprint }) {
  const { create, update } = useSprintMutations(board);
  const [name, setName] = useState("");
  const [goal, setGoal] = useState("");
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (open) {
      setName(sprint?.name ?? "");
      setGoal(sprint?.goal ?? "");
      setStart(sprint?.start_date ?? "");
      setEnd(sprint?.end_date ?? "");
      setError(undefined);
    }
  }, [open, sprint]);

  const closed = sprint?.state === "closed";
  const finish = {
    onSuccess: (saved: Sprint) => {
      toast.success(sprint ? "Sprint saved" : `Created ${saved.name}`);
      onOpenChange(false);
    },
    onError: (e: unknown) => setError(e instanceof ApiError && e.isConflict ? "Someone else changed this sprint. Close this dialog and try again." : reason(e)),
  };
  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setError(undefined);
    if (sprint) {
      const dates = closed ? {} : { start_date: start || null, end_date: end || null };
      update.mutate({ sprint, patch: { name, goal, ...dates } }, finish);
    } else {
      create.mutate({ name, goal, start_date: start, end_date: end }, finish);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={sprint ? `Edit ${sprint.name}` : "Create sprint"}>
        <form onSubmit={submit} className="space-y-4">
          <Field label="Name" hint={sprint ? undefined : "Leave empty to number it automatically."}>
            <Input value={name} onChange={(e) => setName(e.target.value)} maxLength={80} required={Boolean(sprint)} autoFocus />
          </Field>
          <Field label="Goal">
            <Textarea value={goal} onChange={(e) => setGoal(e.target.value)} maxLength={500} rows={2} />
          </Field>
          {!closed && (
            <div className="grid grid-cols-2 gap-3">
              <Field label="Start date"><Input type="date" value={start} onChange={(e) => setStart(e.target.value)} disabled={sprint?.state === "active"} /></Field>
              <Field label="End date"><Input type="date" value={end} onChange={(e) => setEnd(e.target.value)} /></Field>
            </div>
          )}
          {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" loading={create.isPending || update.isPending}>{sprint ? "Save" : "Create sprint"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function StartSprintDialog({ board, sprint, issues, open, onOpenChange }: DialogProps & { sprint: Sprint; issues: BoardCard[] }) {
  const { start } = useSprintMutations(board);
  const [name, setName] = useState(sprint.name);
  const [goal, setGoal] = useState(sprint.goal);
  const [end, setEnd] = useState("");
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (open) {
      setName(sprint.name);
      setGoal(sprint.goal);
      setEnd(sprint.end_date && sprint.end_date >= isoDate(new Date()) ? sprint.end_date : inDays(14));
      setError(undefined);
    }
  }, [open, sprint]);

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setError(undefined);
    start.mutate(
      { id: sprint.id, name, goal, end_date: end },
      {
        onSuccess: (started) => {
          toast.success(`${started.name} has started`);
          onOpenChange(false);
        },
        onError: (e) => setError(reason(e)),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Start sprint" description={`${issues.length} ${issues.length === 1 ? "issue" : "issues"} (${formatPoints(totalPoints(issues))} story points) will be committed to this sprint.`}>
        <form onSubmit={submit} className="space-y-4">
          <Field label="Name"><Input value={name} onChange={(e) => setName(e.target.value)} maxLength={80} required /></Field>
          <Field label="Goal"><Textarea value={goal} onChange={(e) => setGoal(e.target.value)} maxLength={500} rows={2} /></Field>
          <Field label="End date" hint="The sprint starts today."><Input type="date" value={end} min={isoDate(new Date())} onChange={(e) => setEnd(e.target.value)} required /></Field>
          {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" loading={start.isPending}>Start sprint</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

const TO_BACKLOG = "backlog";

export function CompleteSprintDialog({ board, sprint, issues, planned, open, onOpenChange, onCompleted }: DialogProps & { sprint: Sprint; issues: BoardCard[]; planned: Sprint[]; onCompleted?: (sprint: Sprint) => void }) {
  const { complete } = useSprintMutations(board);
  const [destination, setDestination] = useState(TO_BACKLOG);
  const [error, setError] = useState<string>();
  const done = issues.filter((i) => i.status_category === "done").length;
  const open_ = issues.length - done;

  useEffect(() => {
    if (open) {
      setDestination(TO_BACKLOG);
      setError(undefined);
    }
  }, [open]);

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setError(undefined);
    complete.mutate(
      { id: sprint.id, moveTo: destination === TO_BACKLOG ? null : destination },
      {
        onSuccess: (closed) => {
          toast.success(`${closed.name} is complete`);
          onOpenChange(false);
          onCompleted?.(closed);
        },
        onError: (e) => setError(reason(e)),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={`Complete ${sprint.name}`}>
        <form onSubmit={submit} className="space-y-4">
          <dl className="grid grid-cols-2 gap-3">
            <div className="rounded border border-line p-3"><dd className="text-2xl font-semibold text-ink">{done}</dd><dt className="text-sm text-ink-muted">completed {done === 1 ? "issue" : "issues"}</dt></div>
            <div className="rounded border border-line p-3"><dd className="text-2xl font-semibold text-ink">{open_}</dd><dt className="text-sm text-ink-muted">open {open_ === 1 ? "issue" : "issues"}</dt></div>
          </dl>
          {open_ > 0 && (
            <Field label="Move open issues to">
              <Select value={destination} onValueChange={setDestination} options={[{ value: TO_BACKLOG, label: "Backlog" }, ...planned.map((s) => ({ value: s.id, label: s.name }))]} />
            </Field>
          )}
          {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" loading={complete.isPending}>Complete sprint</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
