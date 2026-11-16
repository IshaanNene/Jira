import { Copy, Lock, Plus, Send, Trash2, Webhook as WebhookIcon } from "lucide-react";
import { useState, type FormEvent } from "react";

import {
  Badge,
  Button,
  Card,
  Dialog,
  DialogContent,
  DialogFooter,
  EmptyState,
  ErrorState,
  Field,
  Input,
  MultiSelect,
  Page,
  Skeleton,
  toast,
} from "@/components/ui";
import { can } from "@/modules/organizations";
import { useProjects } from "@/modules/projects";
import { errorMessage, type Webhook, type WebhookKind } from "@/services/api";
import { useSession } from "@/state/session";
import { formatRelative } from "@/utils/format";

import { useWebhookDeliveries, useWebhookEvents, useWebhookMutations, useWebhooks, type WebhookInput } from "../api";

/** Settings → Webhooks: send the workspace's events to other systems. Admins and owners only. */
export default function WebhooksPage() {
  const role = useSession((s) => s.organization?.role);
  if (!can(role, "manageOrganization")) {
    return (
      <Page title="Webhooks">
        <EmptyState icon={Lock} title="Admins only" description="Ask a workspace admin or owner to set up webhooks." />
      </Page>
    );
  }
  return <Webhooks />;
}

function Webhooks() {
  const hooks = useWebhooks();
  const { update, remove, rotate, test } = useWebhookMutations();
  const [editing, setEditing] = useState<Webhook | "new" | null>(null);
  const [logOf, setLogOf] = useState<Webhook | null>(null);
  const [secret, setSecret] = useState<{ name: string; secret: string } | null>(null);
  const fail = (title: string) => (e: unknown) => toast.error(title, errorMessage(e));

  return (
    <Page
      title="Webhooks"
      description="Send issue and comment events to Slack channels or other systems as they happen. Each request to a webhook is signed with its secret."
      actions={<Button variant="primary" onClick={() => setEditing("new")}><Plus className="h-4 w-4" aria-hidden /> New webhook</Button>}
    >
      <Card>
        {hooks.isPending ? (
          <div className="p-4" aria-busy><Skeleton className="h-12" /></div>
        ) : hooks.isError ? (
          <ErrorState error={hooks.error} onRetry={() => void hooks.refetch()} />
        ) : hooks.data.length === 0 ? (
          <EmptyState icon={WebhookIcon} title="No webhooks yet" description="Post to a Slack channel, or connect CI and your own services: they receive a signed POST for each event you choose." />
        ) : (
          <ul className="divide-y divide-line" aria-label="Webhooks">
            {hooks.data.map((hook) => (
              <li key={hook.id} className="flex flex-wrap items-start gap-3 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="font-medium text-ink">{hook.name}</span>
                    {hook.kind === "slack" && <Badge tone="neutral">Slack</Badge>}
                    {hook.enabled ? <Badge tone="success">On</Badge> : <Badge tone={hook.disabled_reason ? "danger" : "neutral"}>Off</Badge>}
                  </div>
                  <p className="truncate font-mono text-xs text-ink-muted">{hook.url}</p>
                  <p className="mt-0.5 text-sm text-ink-muted">
                    {hook.events.join(", ")}
                    {hook.project_ids.length > 0 && ` · ${hook.project_ids.length} ${hook.project_ids.length === 1 ? "project" : "projects"}`}
                  </p>
                  {hook.disabled_reason && <p className="mt-1 text-sm text-danger">{hook.disabled_reason}</p>}
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => update.mutate({ id: hook.id, enabled: !hook.enabled }, { onError: fail("Couldn't change the webhook") })}
                >
                  {hook.enabled ? "Turn off" : "Turn on"}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setEditing(hook)}>Edit</Button>
                <Button size="sm" variant="ghost" onClick={() => setLogOf(hook)}>Deliveries</Button>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`Send a test to ${hook.name}`}
                  disabled={!hook.enabled}
                  onClick={() => test.mutate(hook.id, { onSuccess: () => toast.success("Test queued", "It is sent within a few seconds; see Deliveries."), onError: fail("Couldn't send a test") })}
                >
                  <Send className="h-4 w-4" aria-hidden />
                </Button>
                {hook.kind !== "slack" && <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    if (window.confirm(`Replace the signing secret of "${hook.name}"? The current secret stops working at once.`)) {
                      rotate.mutate(hook.id, { onSuccess: (r) => r.secret && setSecret({ name: hook.name, secret: r.secret }), onError: fail("Couldn't replace the secret") });
                    }
                  }}
                >
                  New secret
                </Button>}
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`Delete ${hook.name}`}
                  onClick={() => {
                    if (window.confirm(`Delete the webhook "${hook.name}"? Nothing more is sent to it.`)) remove.mutate(hook.id, { onError: fail("Couldn't delete the webhook") });
                  }}
                >
                  <Trash2 className="h-4 w-4" aria-hidden />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <SigningHelp />
      {editing && (
        <WebhookDialog
          hook={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onCreated={(name, s) => (s ? setSecret({ name, secret: s }) : toast.success("Channel connected", `${name} now gets the events you chose.`))}
        />
      )}
      {logOf && <DeliveriesDialog hook={logOf} onClose={() => setLogOf(null)} />}
      {secret && <SecretDialog {...secret} onClose={() => setSecret(null)} />}
    </Page>
  );
}

function SigningHelp() {
  return (
    <Card className="mt-4 p-4 text-sm text-ink-muted">
      <h2 className="font-semibold text-ink">Verifying requests</h2>
      <p className="mt-1">
        Each request carries <code className="font-mono">X-Meridian-Signature: t=&lt;unix time&gt;,v1=&lt;hex&gt;</code>, where the hex is the
        HMAC-SHA256 of <code className="font-mono">&lt;t&gt;.&lt;raw body&gt;</code> with the webhook&apos;s secret. Reject requests whose
        signature does not match or whose time is more than five minutes old. Answer with any 2xx; other answers are retried for about half an
        hour, and <code className="font-mono">410 Gone</code> turns the webhook off.
      </p>
    </Card>
  );
}

function WebhookDialog({ hook, onClose, onCreated }: { hook: Webhook | null; onClose: () => void; onCreated: (name: string, secret: string | null) => void }) {
  const { create, update } = useWebhookMutations();
  const events = useWebhookEvents();
  const projects = useProjects();
  const [input, setInput] = useState<WebhookInput>({
    name: hook?.name ?? "",
    // A Slack channel's address is a credential: it is never shown again, only replaced.
    url: hook?.kind === "slack" ? "" : (hook?.url ?? "https://"),
    kind: hook ? (hook.kind as WebhookKind) : "webhook",
    events: hook?.events ?? ["issue.created"],
    project_ids: hook?.project_ids ?? [],
  });
  const [error, setError] = useState<string | null>(null);
  const slack = input.kind === "slack";
  const toggle = (name: string) =>
    setInput({ ...input, events: input.events.includes(name) ? input.events.filter((e) => e !== name) : [...input.events, name] });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    const onError = (err: unknown) => setError(errorMessage(err));
    if (hook) {
      // The kind is fixed once made; an empty Slack URL keeps the stored one.
      const { name, events, project_ids, url } = input;
      update.mutate({ id: hook.id, name, events, project_ids, ...(url.trim() ? { url } : {}) }, { onSuccess: onClose, onError });
    }
    else
      create.mutate(input, {
        onSuccess: (r) => {
          onClose();
          onCreated(r.webhook.name, r.secret ?? null);
        },
        onError,
      });
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={hook ? "Edit webhook" : "New webhook"} className="max-w-lg">
        <form onSubmit={submit} className="space-y-4">
          <Field label="Name">
            <Input value={input.name} maxLength={100} onChange={(e) => setInput({ ...input, name: e.target.value })} placeholder="e.g. Deploy bot" />
          </Field>
          {!hook && (
            <fieldset>
              <legend className="text-sm font-medium text-ink">Send to</legend>
              <div className="mt-2 flex gap-4 text-sm">
                {KINDS.map((k) => (
                  <label key={k.value} className="flex items-center gap-2">
                    <input
                      type="radio"
                      name="kind"
                      checked={input.kind === k.value}
                      onChange={() => setInput({ ...input, kind: k.value, url: k.value === "slack" ? "" : "https://" })}
                    />
                    {k.label}
                  </label>
                ))}
              </div>
            </fieldset>
          )}
          {slack ? (
            <Field
              label="Slack webhook URL"
              hint={hook ? `Now ${hook.url}. Leave empty to keep it.` : "From a Slack app's Incoming Webhooks page: https://hooks.slack.com/services/…"}
            >
              <Input value={input.url} maxLength={2000} autoComplete="off" onChange={(e) => setInput({ ...input, url: e.target.value })} />
            </Field>
          ) : (
            <Field label="Endpoint URL" hint="HTTPS, on a public address.">
              <Input value={input.url} maxLength={2000} onChange={(e) => setInput({ ...input, url: e.target.value })} />
            </Field>
          )}
          <fieldset>
            <legend className="text-sm font-medium text-ink">Events</legend>
            <div className="mt-2 grid gap-1.5 sm:grid-cols-2">
              {(events.data ?? []).map((ev) => (
                <label key={ev.name} className="flex items-start gap-2 text-sm">
                  <input type="checkbox" className="mt-0.5" checked={input.events.includes(ev.name)} onChange={() => toggle(ev.name)} />
                  <span>
                    <span className="font-mono text-xs text-ink">{ev.name}</span>
                    <span className="block text-xs text-ink-muted">{ev.description}</span>
                  </span>
                </label>
              ))}
            </div>
          </fieldset>
          <div className="space-y-1.5">
            <span className="block text-sm font-medium text-ink">Projects</span>
            <MultiSelect
              aria-label="Projects"
              placeholder="All projects"
              values={input.project_ids}
              onChange={(v) => setInput({ ...input, project_ids: v })}
              options={(projects.data ?? []).map((p) => ({ value: p.id, label: `${p.key} · ${p.name}` }))}
            />
          </div>
          {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={!input.name.trim() || input.events.length === 0 || (!hook && !input.url.trim())} loading={create.isPending || update.isPending}>
              {hook ? "Save" : slack ? "Connect channel" : "Create webhook"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

const KINDS: { value: WebhookKind; label: string }[] = [
  { value: "webhook", label: "Webhook" },
  { value: "slack", label: "Slack channel" },
];

function SecretDialog({ name, secret, onClose }: { name: string; secret: string; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title="Signing secret" description={`Store the secret for "${name}" now: it is not shown again.`}>
        <div className="flex gap-2">
          <Input readOnly value={secret} aria-label="Signing secret" className="font-mono text-xs" onFocus={(e) => e.target.select()} />
          <Button
            aria-label="Copy secret"
            onClick={() => void navigator.clipboard?.writeText(secret).then(() => setCopied(true))}
          >
            <Copy className="h-4 w-4" aria-hidden /> {copied ? "Copied" : "Copy"}
          </Button>
        </div>
        <DialogFooter>
          <Button variant="primary" onClick={onClose}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

const STATUS = {
  delivered: { label: "Delivered", tone: "success" },
  pending: { label: "Retrying", tone: "warning" },
  sending: { label: "Sending", tone: "neutral" },
  failed: { label: "Failed", tone: "danger" },
  skipped: { label: "Skipped", tone: "neutral" },
} as const;

function DeliveriesDialog({ hook, onClose }: { hook: Webhook; onClose: () => void }) {
  const log = useWebhookDeliveries(hook.id);
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={`Deliveries: ${hook.name}`} description="The latest 50, newest first. Kept for a week." className="max-w-xl">
        {log.isPending ? (
          <Skeleton className="h-16" />
        ) : log.isError ? (
          <ErrorState error={log.error} onRetry={() => void log.refetch()} />
        ) : log.data.length === 0 ? (
          <p className="text-sm text-ink-muted">Nothing sent yet. Send a test to check the endpoint.</p>
        ) : (
          <ul className="max-h-96 divide-y divide-line overflow-auto" aria-label="Deliveries">
            {log.data.map((d) => {
              const status = STATUS[d.status as keyof typeof STATUS] ?? STATUS.sending;
              return (
                <li key={d.id} className="py-2 text-sm">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-xs text-ink">{d.event_type}</span>
                    <Badge tone={status.tone}>{status.label}</Badge>
                    {d.response_status != null && <span className="text-xs text-ink-subtle">HTTP {d.response_status}</span>}
                    <span className="text-xs text-ink-subtle">{d.attempts} {d.attempts === 1 ? "attempt" : "attempts"}</span>
                    <span className="ml-auto text-xs text-ink-subtle">{formatRelative(d.created_at)}</span>
                  </div>
                  {d.last_error && <p className="mt-0.5 text-ink-muted">{d.last_error}</p>}
                </li>
              );
            })}
          </ul>
        )}
      </DialogContent>
    </Dialog>
  );
}
