import { Copy, GitPullRequest, Lock, Plus, Trash2 } from "lucide-react";
import { useState, type FormEvent } from "react";

import { Badge, Button, Card, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Field, Input, Page, Skeleton, toast } from "@/components/ui";
import { can } from "@/modules/organizations";
import { useProjects } from "@/modules/projects";
import { errorMessage, type SourceConnection } from "@/services/api";
import { useSession } from "@/state/session";
import { formatRelative } from "@/utils/format";

import { useSourceConnectionMutations, useSourceConnections, type SourceConnectionInput } from "../api";

/** Settings → Source control: link GitHub commits, branches and pull requests to issues. Admins and owners only. */
export default function SourceControlPage() {
  const role = useSession((s) => s.organization?.role);
  if (!can(role, "manageOrganization")) {
    return (
      <Page title="Source control">
        <EmptyState icon={Lock} title="Admins only" description="Ask a workspace admin or owner to connect a repository." />
      </Page>
    );
  }
  return <SourceControl />;
}

/** Where the provider sends deliveries: the API is served from the app's own origin. */
const payloadUrl = (c: SourceConnection) => `${window.location.origin}${c.payload_path}`;

function SourceControl() {
  const connections = useSourceConnections();
  const { remove, rotate } = useSourceConnectionMutations();
  const [creating, setCreating] = useState(false);
  const [setup, setSetup] = useState<{ connection: SourceConnection; secret: string } | null>(null);
  const fail = (title: string) => (e: unknown) => toast.error(title, errorMessage(e));

  return (
    <Page
      title="Source control"
      description="Link GitHub commits, branches and pull requests to the issues they name. Mention an issue key, like PAY-12, in a branch name, commit message or pull request title."
      actions={<Button variant="primary" onClick={() => setCreating(true)}><Plus className="h-4 w-4" aria-hidden /> Connect GitHub</Button>}
    >
      <Card>
        {connections.isPending ? (
          <div className="p-4" aria-busy><Skeleton className="h-12" /></div>
        ) : connections.isError ? (
          <ErrorState error={connections.error} onRetry={() => void connections.refetch()} />
        ) : connections.data.length === 0 ? (
          <EmptyState icon={GitPullRequest} title="No repositories connected" description="Add a GitHub webhook to a repository or organization, and its work shows on the issues it mentions." />
        ) : (
          <ul className="divide-y divide-line" aria-label="Connections">
            {connections.data.map((c) => (
              <li key={c.id} className="flex flex-wrap items-start gap-3 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="font-medium text-ink">{c.name}</span>
                    <Badge tone="neutral">GitHub</Badge>
                  </div>
                  <p className="truncate font-mono text-xs text-ink-muted">{payloadUrl(c)}</p>
                  <p className="mt-0.5 text-sm text-ink-muted">
                    {c.project_keys.join(", ")} · {c.last_event_at ? `last delivery ${formatRelative(c.last_event_at)}` : "nothing received yet"}
                  </p>
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    if (window.confirm(`Replace the secret of "${c.name}"? GitHub's deliveries are refused until you enter the new one there.`)) {
                      rotate.mutate(c.id, { onSuccess: (r) => setSetup(r), onError: fail("Couldn't replace the secret") });
                    }
                  }}
                >
                  New secret
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`Disconnect ${c.name}`}
                  onClick={() => {
                    if (window.confirm(`Disconnect "${c.name}"? Its links are removed from issues.`)) remove.mutate(c.id, { onError: fail("Couldn't disconnect") });
                  }}
                >
                  <Trash2 className="h-4 w-4" aria-hidden />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
      {creating && <ConnectDialog onClose={() => setCreating(false)} onCreated={setSetup} />}
      {setup && <SetupDialog {...setup} onClose={() => setSetup(null)} />}
    </Page>
  );
}

function ConnectDialog({ onClose, onCreated }: { onClose: () => void; onCreated: (r: { connection: SourceConnection; secret: string }) => void }) {
  const { create } = useSourceConnectionMutations();
  const projects = useProjects();
  const [input, setInput] = useState<SourceConnectionInput>({ provider: "github", name: "", project_keys: [] });
  const [error, setError] = useState<string | null>(null);
  const toggle = (key: string) =>
    setInput({ ...input, project_keys: input.project_keys.includes(key) ? input.project_keys.filter((k) => k !== key) : [...input.project_keys, key] });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    create.mutate(input, {
      onSuccess: (r) => {
        onClose();
        onCreated(r);
      },
      onError: (err) => setError(errorMessage(err)),
    });
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title="Connect GitHub" className="max-w-lg">
        <form onSubmit={submit} className="space-y-4">
          <Field label="Name" hint="Usually the repository or GitHub organization.">
            <Input value={input.name} maxLength={100} onChange={(e) => setInput({ ...input, name: e.target.value })} placeholder="e.g. acme/payments" />
          </Field>
          <fieldset>
            <legend className="text-sm font-medium text-ink">Projects</legend>
            <p className="text-xs text-ink-muted">Only keys of these projects are linked.</p>
            <div className="mt-2 grid max-h-56 gap-1.5 overflow-auto sm:grid-cols-2">
              {(projects.data ?? []).filter((p) => p.status !== "archived").map((p) => (
                <label key={p.id} className="flex items-center gap-2 text-sm">
                  <input type="checkbox" checked={input.project_keys.includes(p.key)} onChange={() => toggle(p.key)} />
                  <span><span className="font-mono text-xs text-ink">{p.key}</span> {p.name}</span>
                </label>
              ))}
            </div>
          </fieldset>
          {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={!input.name.trim() || input.project_keys.length === 0} loading={create.isPending}>
              Connect
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function CopyField({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-1.5">
      <span className="block text-sm font-medium text-ink">{label}</span>
      <div className="flex gap-2">
        <Input readOnly value={value} aria-label={label} className="font-mono text-xs" onFocus={(e) => e.target.select()} />
        <Button aria-label={`Copy ${label.toLowerCase()}`} onClick={() => void navigator.clipboard?.writeText(value).then(() => setCopied(true))}>
          <Copy className="h-4 w-4" aria-hidden /> {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  );
}

function SetupDialog({ connection, secret, onClose }: { connection: SourceConnection; secret: string; onClose: () => void }) {
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title="Finish in GitHub" description="Store the secret now: it is not shown again." className="max-w-lg">
        <ol className="list-decimal space-y-1 pl-5 text-sm text-ink-muted">
          <li>In the repository or organization, open Settings → Webhooks → Add webhook.</li>
          <li>Paste the payload URL and the secret below, and choose the content type <code className="font-mono">application/json</code>.</li>
          <li>Choose “Let me select individual events” and tick Pushes and Pull requests.</li>
        </ol>
        <div className="mt-4 space-y-3">
          <CopyField label="Payload URL" value={payloadUrl(connection)} />
          <CopyField label="Secret" value={secret} />
        </div>
        <DialogFooter>
          <Button variant="primary" onClick={onClose}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
