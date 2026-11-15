import { Check, Copy, KeyRound, Plus } from "lucide-react";
import { useState, type FormEvent } from "react";

import { Button, Card, CardHeader, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Field, Input, Select, Skeleton, toast } from "@/components/ui";
import { useApiTokens, useCreateApiToken, useRevokeApiToken } from "@/modules/auth";
import { errorMessage, type ApiToken } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate, formatRelative } from "@/utils/format";

import { fieldMessage, generalMessage } from "./TwoStepVerification";

const LIFETIMES = [
  { value: "7", label: "7 days" },
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "365", label: "1 year" },
];

/** Personal API tokens for the organization the person is working in. */
export function ApiTokens() {
  const organization = useSession((s) => s.organization);
  return (
    <Card>
      {organization ? (
        <TokenList organizationId={organization.id} />
      ) : (
        <>
          <CardHeader title="API tokens" description="Let scripts and integrations act as you in one organization." />
          <p className="px-4 py-4 text-sm text-ink-muted">Open an organization to manage its API tokens.</p>
        </>
      )}
    </Card>
  );
}

function TokenList({ organizationId }: { organizationId: string }) {
  const tokens = useApiTokens(organizationId);
  const revoke = useRevokeApiToken(organizationId);
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<ApiToken | null>(null);

  return (
    <>
      <CardHeader
        title="API tokens"
        description="Let scripts and integrations act as you in this organization. A token can do what you can, except manage your sign-in."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="h-4 w-4" aria-hidden />
            Create token
          </Button>
        }
      />
      {tokens.isPending ? (
        <div className="space-y-2 p-4" aria-busy><Skeleton className="h-12" /></div>
      ) : tokens.isError ? (
        <ErrorState error={tokens.error} onRetry={() => void tokens.refetch()} />
      ) : tokens.data.items.length === 0 ? (
        <EmptyState icon={KeyRound} title="No API tokens" description="Create one for a script or an integration. You can revoke it at any time." />
      ) : (
        <ul aria-label="API tokens" className="divide-y divide-line">
          {tokens.data.items.map((token) => (
            <li key={token.id} className="flex items-center gap-3 px-4 py-3">
              <KeyRound className="h-5 w-5 shrink-0 text-ink-subtle" aria-hidden />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-base font-medium text-ink">{token.name}</span>
                  <code className="shrink-0 rounded bg-sunken px-1.5 text-xs text-ink-muted">{token.hint}…</code>
                </div>
                <div className="truncate text-sm text-ink-muted">
                  {token.last_used_at ? `Used ${formatRelative(token.last_used_at)}` : "Never used"} · Created {formatDate(token.created_at)} · Expires{" "}
                  {formatDate(token.expires_at)}
                </div>
              </div>
              <Button size="sm" onClick={() => setRevoking(token)} aria-label={`Revoke ${token.name}`}>
                Revoke
              </Button>
            </li>
          ))}
        </ul>
      )}

      <Dialog open={creating} onOpenChange={setCreating}>
        {creating && <CreateDialog organizationId={organizationId} onDone={() => setCreating(false)} />}
      </Dialog>
      <Dialog open={revoking !== null} onOpenChange={(open) => !open && setRevoking(null)}>
        {revoking && (
          <DialogContent title={`Revoke "${revoking.name}"?`} description="Anything using it stops working within ten minutes. This can't be undone.">
            <DialogFooter>
              <Button onClick={() => setRevoking(null)}>Cancel</Button>
              <Button
                variant="danger"
                loading={revoke.isPending}
                onClick={() =>
                  revoke.mutate(revoking.id, {
                    onSuccess: () => {
                      toast.success("Token revoked");
                      setRevoking(null);
                    },
                    onError: (error) => toast.error("Couldn't revoke token", errorMessage(error)),
                  })
                }
              >
                Revoke
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </>
  );
}

function CreateDialog({ organizationId, onDone }: { organizationId: string; onDone: () => void }) {
  const create = useCreateApiToken(organizationId);
  const [name, setName] = useState("");
  const [days, setDays] = useState("30");

  if (create.data) {
    return (
      <DialogContent title="Copy your new token" description="This is the only time it is shown. Store it somewhere safe, like a secrets manager.">
        <SecretOnce secret={create.data.secret} onDone={onDone} />
      </DialogContent>
    );
  }
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (name.trim()) create.mutate({ name: name.trim(), expires_in_days: Number(days) });
  };
  return (
    <DialogContent title="Create an API token" description="Exchange it at POST /api/v1/auth/token/api for a short-lived access token.">
      <form noValidate onSubmit={submit} className="space-y-4">
        {generalMessage(create.error) && <p role="alert" className="text-sm text-danger">{generalMessage(create.error)}</p>}
        <Field label="Name" hint="What it's for, so you can tell tokens apart." error={fieldMessage(create.error, "name")}>
          <Input autoFocus maxLength={100} placeholder="Release script" value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field label="Expires after" error={fieldMessage(create.error, "expires_in_days")}>
          <Select value={days} onValueChange={setDays} options={LIFETIMES} />
        </Field>
        <DialogFooter>
          <Button type="submit" variant="primary" loading={create.isPending} disabled={!name.trim()}>
            Create
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

function SecretOnce({ secret, onDone }: { secret: string; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(secret);
      setCopied(true);
    } catch {
      toast.error("Couldn't copy", "Select the token and copy it instead.");
    }
  };
  return (
    <div className="space-y-4">
      <p aria-label="New API token" className="break-all rounded border border-line bg-sunken px-3 py-2 font-mono text-sm text-ink">
        {secret}
      </p>
      <Button size="sm" onClick={() => void copy()}>
        {copied ? <Check className="h-4 w-4" aria-hidden /> : <Copy className="h-4 w-4" aria-hidden />}
        {copied ? "Copied" : "Copy"}
      </Button>
      <DialogFooter>
        <Button variant="primary" onClick={onDone}>
          Done
        </Button>
      </DialogFooter>
    </div>
  );
}
