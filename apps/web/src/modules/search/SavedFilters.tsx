import { ChevronDown, Filter, MoreHorizontal, Users } from "lucide-react";
import { useState, type FormEvent } from "react";

import {
  Badge,
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Field,
  Input,
  Select,
  toast,
} from "@/components/ui";
import { ApiError, errorMessage, type FilterSubscription, type PutFilterSubscription, type SavedFilter } from "@/services/api";
import { useSession } from "@/state/session";

import {
  useCreateSavedFilter,
  useDeleteFilterSubscription,
  useDeleteSavedFilter,
  useFilterSubscription,
  useSaveFilterSubscription,
  useSavedFilters,
  useUpdateSavedFilter,
} from "./api";

function nameError(error: unknown): string | undefined {
  if (error instanceof ApiError) return error.fieldErrors.find((e) => e.field === "name")?.message;
  return undefined;
}

function otherError(error: unknown): string | null {
  if (!error || nameError(error)) return null;
  if (error instanceof ApiError) return error.fieldErrors.find((e) => e.field === "query")?.message ?? errorMessage(error);
  return errorMessage(error);
}

/**
 * Open, save and manage saved MQL filters. `current` is the open filter, if any; `query` is the
 * query that last ran, which is what gets saved.
 */
export function SavedFilters({
  current,
  query,
  onOpen,
  onClose,
}: {
  current: SavedFilter | null;
  query: string;
  onOpen: (filter: SavedFilter) => void;
  onClose: () => void;
}) {
  const me = useSession((s) => s.account?.id);
  const filters = useSavedFilters();
  const update = useUpdateSavedFilter();
  const [dialog, setDialog] = useState<"create" | "rename" | "delete" | "subscribe" | null>(null);
  const close = () => setDialog(null);

  const own = filters.data?.items.filter((f) => f.owner_id === me) ?? [];
  const shared = filters.data?.items.filter((f) => f.owner_id !== me) ?? [];
  const owner = current !== null && current.owner_id === me;
  const changed = current !== null && query !== current.query;

  const save = (change: { query?: string; shared?: boolean }, done: string) => {
    if (!current) return;
    update.mutate(
      { id: current.id, ...change },
      { onSuccess: (f) => { toast.success(done); onOpen(f); }, onError: (e) => toast.error("Couldn't save the filter", otherError(e) ?? errorMessage(e)) },
    );
  };

  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size="sm">
            <Filter className="h-4 w-4" aria-hidden />
            Filters
            <ChevronDown className="h-3.5 w-3.5" aria-hidden />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-96 w-72 overflow-y-auto">
          {filters.isPending ? (
            <DropdownMenuLabel>Loading…</DropdownMenuLabel>
          ) : filters.isError ? (
            <DropdownMenuLabel>Couldn't load filters</DropdownMenuLabel>
          ) : own.length + shared.length === 0 ? (
            <DropdownMenuLabel>No saved filters yet. Run a query, then save it.</DropdownMenuLabel>
          ) : (
            <>
              {own.length > 0 && (
                <DropdownMenuGroup>
                  <DropdownMenuLabel>My filters</DropdownMenuLabel>
                  {own.map((f) => (
                    <DropdownMenuItem key={f.id} selected={f.id === current?.id} onSelect={() => onOpen(f)}>
                      <span className="truncate">{f.name}</span>
                      {f.shared && <Users className="ml-auto h-3.5 w-3.5 shrink-0 text-ink-subtle" aria-label="Shared" />}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuGroup>
              )}
              {own.length > 0 && shared.length > 0 && <DropdownMenuSeparator />}
              {shared.length > 0 && (
                <DropdownMenuGroup>
                  <DropdownMenuLabel>Shared with the workspace</DropdownMenuLabel>
                  {shared.map((f) => (
                    <DropdownMenuItem key={f.id} selected={f.id === current?.id} onSelect={() => onOpen(f)}>
                      <span className="truncate">{f.name}</span>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuGroup>
              )}
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      {current ? (
        <>
          <h2 className="min-w-0 truncate text-base font-medium text-ink">{current.name}</h2>
          {current.shared && <Badge>Shared</Badge>}
          {changed && <span className="text-sm text-ink-subtle">Edited</span>}
          <div className="ml-auto flex items-center gap-2">
            {changed && current.editable && (
              <Button size="sm" variant="primary" loading={update.isPending} onClick={() => save({ query }, "Filter saved")}>
                Save changes
              </Button>
            )}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="sm" variant="ghost" aria-label="Filter actions">
                  <MoreHorizontal className="h-4 w-4" aria-hidden />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem disabled={!query} onSelect={() => setDialog("create")}>
                  Save as new filter
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={() => setDialog("subscribe")}>Email subscription…</DropdownMenuItem>
                {current.editable && <DropdownMenuItem onSelect={() => setDialog("rename")}>Rename</DropdownMenuItem>}
                {owner && (
                  <DropdownMenuItem onSelect={() => save({ shared: !current.shared }, current.shared ? "Only you can see this filter now" : "Filter shared with the workspace")}>
                    {current.shared ? "Stop sharing" : "Share with the workspace"}
                  </DropdownMenuItem>
                )}
                <DropdownMenuItem onSelect={onClose}>Close filter</DropdownMenuItem>
                {current.editable && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem tone="danger" onSelect={() => setDialog("delete")}>
                      Delete
                    </DropdownMenuItem>
                  </>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </>
      ) : (
        <Button size="sm" className="ml-auto" disabled={!query} onClick={() => setDialog("create")}>
          Save filter
        </Button>
      )}

      <Dialog open={dialog === "create"} onOpenChange={(open) => !open && close()}>
        {dialog === "create" && <CreateDialog query={query} onDone={(f) => { close(); onOpen(f); }} />}
      </Dialog>
      <Dialog open={dialog === "rename"} onOpenChange={(open) => !open && close()}>
        {dialog === "rename" && current && <RenameDialog filter={current} onDone={(f) => { close(); onOpen(f); }} />}
      </Dialog>
      <Dialog open={dialog === "subscribe"} onOpenChange={(open) => !open && close()}>
        {dialog === "subscribe" && current && <SubscribeDialog filter={current} onDone={close} />}
      </Dialog>
      <Dialog open={dialog === "delete"} onOpenChange={(open) => !open && close()}>
        {dialog === "delete" && current && <DeleteDialog filter={current} onCancel={close} onDone={() => { close(); onClose(); }} />}
      </Dialog>
    </div>
  );
}

function CreateDialog({ query, onDone }: { query: string; onDone: (filter: SavedFilter) => void }) {
  const create = useCreateSavedFilter();
  const [name, setName] = useState("");
  const [shared, setShared] = useState(false);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (name.trim()) create.mutate({ name: name.trim(), query, shared }, { onSuccess: (f) => { toast.success("Filter saved"); onDone(f); } });
  };
  return (
    <DialogContent title="Save filter" description="Saves the query that last ran. Anyone who runs it sees only the issues they have access to.">
      <form noValidate onSubmit={submit} className="space-y-4">
        {otherError(create.error) && <p role="alert" className="text-sm text-danger">{otherError(create.error)}</p>}
        <p className="break-words rounded border border-line bg-sunken px-3 py-2 font-mono text-xs text-ink-muted">{query}</p>
        <Field label="Name" error={nameError(create.error)}>
          <Input autoFocus maxLength={80} placeholder="My open bugs" value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <label className="flex items-center gap-2 text-sm text-ink">
          <input type="checkbox" checked={shared} onChange={(e) => setShared(e.target.checked)} className="h-4 w-4 accent-[hsl(var(--accent))]" />
          Share with everyone in the workspace
        </label>
        <DialogFooter>
          <Button type="submit" variant="primary" loading={create.isPending} disabled={!name.trim()}>
            Save
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

function RenameDialog({ filter, onDone }: { filter: SavedFilter; onDone: (filter: SavedFilter) => void }) {
  const update = useUpdateSavedFilter();
  const [name, setName] = useState(filter.name);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (name.trim()) update.mutate({ id: filter.id, name: name.trim() }, { onSuccess: onDone });
  };
  return (
    <DialogContent title="Rename filter">
      <form noValidate onSubmit={submit} className="space-y-4">
        {otherError(update.error) && <p role="alert" className="text-sm text-danger">{otherError(update.error)}</p>}
        <Field label="Name" error={nameError(update.error)}>
          <Input autoFocus maxLength={80} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <DialogFooter>
          <Button type="submit" variant="primary" loading={update.isPending} disabled={!name.trim()}>
            Rename
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

function DeleteDialog({ filter, onCancel, onDone }: { filter: SavedFilter; onCancel: () => void; onDone: () => void }) {
  const remove = useDeleteSavedFilter();
  return (
    <DialogContent
      title={`Delete "${filter.name}"?`}
      description={filter.shared ? "It is shared, so it disappears for everyone in the workspace. This can't be undone." : "This can't be undone."}
    >
      <DialogFooter>
        <Button onClick={onCancel}>Cancel</Button>
        <Button
          variant="danger"
          loading={remove.isPending}
          onClick={() =>
            remove.mutate(filter.id, {
              onSuccess: () => { toast.success("Filter deleted"); onDone(); },
              onError: (e) => toast.error("Couldn't delete the filter", errorMessage(e)),
            })
          }
        >
          Delete
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

const WEEKDAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const HOURS = Array.from({ length: 24 }, (_, h) => ({ value: String(h), label: `${String(h).padStart(2, "0")}:00 UTC` }));

/** Email the filter's results to oneself every day or week, at a UTC hour. */
function SubscribeDialog({ filter, onDone }: { filter: SavedFilter; onDone: () => void }) {
  const current = useFilterSubscription(filter.id);
  return (
    <DialogContent
      title="Email subscription"
      description={`Get "${filter.name}" by email: the issues it matches that you can see, at most 20 listed.`}
    >
      {current.isPending ? (
        <p className="text-sm text-ink-subtle">Loading…</p>
      ) : current.isError ? (
        <p role="alert" className="text-sm text-danger">{errorMessage(current.error)}</p>
      ) : (
        <SubscribeForm filterId={filter.id} initial={current.data} onDone={onDone} />
      )}
    </DialogContent>
  );
}

function SubscribeForm({
  filterId,
  initial,
  onDone,
}: {
  filterId: string;
  initial: FilterSubscription | null;
  onDone: () => void;
}) {
  const save = useSaveFilterSubscription(filterId);
  const remove = useDeleteFilterSubscription(filterId);
  const [frequency, setFrequency] = useState<"daily" | "weekly">(initial?.frequency ?? "daily");
  const [hour, setHour] = useState(String(initial?.hour ?? 9));
  const [weekday, setWeekday] = useState(String(initial?.weekday ?? 1));
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const body: PutFilterSubscription = { frequency, hour: Number(hour), weekday: frequency === "weekly" ? Number(weekday) : null };
    save.mutate(body, {
      onSuccess: (sub) => {
        toast.success(initial ? "Subscription updated" : "Subscribed", `Next email ${new Date(sub.next_run_at).toLocaleString()}.`);
        onDone();
      },
    });
  };
  const error = save.error ?? remove.error;
  return (
    <form noValidate onSubmit={submit} className="space-y-4">
      {error && <p role="alert" className="text-sm text-danger">{errorMessage(error)}</p>}
      <Field label="How often">
        <Select
          value={frequency}
          onValueChange={(v) => setFrequency(v as "daily" | "weekly")}
          options={[{ value: "daily", label: "Every day" }, { value: "weekly", label: "Every week" }]}
        />
      </Field>
      {frequency === "weekly" && (
        <Field label="Day">
          <Select value={weekday} onValueChange={setWeekday} options={WEEKDAYS.map((d, i) => ({ value: String(i + 1), label: d }))} />
        </Field>
      )}
      <Field label="Time">
        <Select value={hour} onValueChange={setHour} options={HOURS} />
      </Field>
      {initial && <p className="text-sm text-ink-subtle">Next email {new Date(initial.next_run_at).toLocaleString()}.</p>}
      <DialogFooter>
        {initial && (
          <Button
            type="button"
            variant="danger"
            loading={remove.isPending}
            onClick={() => remove.mutate(undefined, { onSuccess: () => { toast.success("Unsubscribed"); onDone(); } })}
          >
            Unsubscribe
          </Button>
        )}
        <Button type="submit" variant="primary" loading={save.isPending}>
          {initial ? "Save" : "Subscribe"}
        </Button>
      </DialogFooter>
    </form>
  );
}
