import { Check, Copy, KeyRound, Lock, Plus, UserPlus } from "lucide-react";
import { useState, type FormEvent } from "react";

import { Badge, Button, Card, CardHeader, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Field, Input, Page, Skeleton, toast } from "@/components/ui";
import { can } from "@/modules/organizations";
import { ApiError, errorMessage, type ProvisionedUser, type ProvisioningToken } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate, formatRelative } from "@/utils/format";

import { useProvisionedUsers, useProvisioningTokenMutations, useProvisioningTokens } from "../api";

/** Settings → Provisioning: let the identity provider add, update and remove people with SCIM. */
export default function ProvisioningPage() {
  const role = useSession((s) => s.organization?.role);
  if (!can(role, "manageMembers")) {
    return (
      <Page title="Provisioning">
        <EmptyState icon={Lock} title="Admins only" description="Ask a workspace admin or owner to set up provisioning." />
      </Page>
    );
  }
  return (
    <Page
      title="Provisioning"
      description="Let your identity provider (Okta, Microsoft Entra ID, OneLogin, or any SCIM 2.0 client) invite, suspend and remove people as they join, change roles or leave."
      width="narrow"
    >
      <div className="space-y-5">
        <Tokens />
        <People />
      </div>
    </Page>
  );
}

function copy(text: string) {
  navigator.clipboard?.writeText(text).then(
    () => toast.success("Copied"),
    () => toast.error("Couldn't copy", "Select the text and copy it instead."),
  );
}

function Tokens() {
  const tokens = useProvisioningTokens();
  const { revoke } = useProvisioningTokenMutations();
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<ProvisioningToken | null>(null);

  return (
    <Card>
      <CardHeader
        title="SCIM connection"
        description="Give your provider the base URL and a token. A token acts with your role: if you stop being an admin, it stops working."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="h-4 w-4" aria-hidden /> Create token
          </Button>
        }
      />
      {tokens.isPending ? (
        <div className="space-y-2 p-4" aria-busy><Skeleton className="h-12" /></div>
      ) : tokens.isError ? (
        <ErrorState error={tokens.error} onRetry={() => void tokens.refetch()} />
      ) : (
        <>
          <div className="flex items-center gap-2 border-b border-line px-4 py-3">
            <span className="shrink-0 text-sm text-ink-muted">Base URL</span>
            <code className="min-w-0 flex-1 truncate rounded bg-sunken px-2 py-1 text-sm" aria-label="SCIM base URL">{tokens.data.base_url}</code>
            <Button size="sm" aria-label="Copy base URL" onClick={() => copy(tokens.data.base_url)}>
              <Copy className="h-4 w-4" aria-hidden />
            </Button>
          </div>
          {tokens.data.items.length === 0 ? (
            <EmptyState icon={KeyRound} title="No provisioning tokens" description="Create one and paste it into your provider's SCIM settings as the bearer token." />
          ) : (
            <ul aria-label="Provisioning tokens" className="divide-y divide-line">
              {tokens.data.items.map((token) => (
                <li key={token.id} className="flex items-center gap-3 px-4 py-3">
                  <KeyRound className="h-5 w-5 shrink-0 text-ink-subtle" aria-hidden />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-base font-medium text-ink">{token.name}</span>
                      <code className="shrink-0 rounded bg-sunken px-1.5 text-xs text-ink-muted">{token.prefix}…</code>
                    </div>
                    <div className="truncate text-sm text-ink-muted">
                      {token.last_used_at ? `Used ${formatRelative(token.last_used_at)}` : "Never used"} · Created by {token.created_by_name ?? "a former member"} on{" "}
                      {formatDate(token.created_at)}
                    </div>
                  </div>
                  <Button size="sm" onClick={() => setRevoking(token)} aria-label={`Revoke ${token.name}`}>
                    Revoke
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
      <Dialog open={creating} onOpenChange={setCreating}>{creating && <CreateDialog onDone={() => setCreating(false)} />}</Dialog>
      <Dialog open={revoking !== null} onOpenChange={(open) => !open && setRevoking(null)}>
        {revoking && (
          <DialogContent title={`Revoke "${revoking.name}"?`} description="Your provider can no longer provision anyone with it. People already provisioned stay as they are.">
            <DialogFooter>
              <Button onClick={() => setRevoking(null)}>Cancel</Button>
              <Button
                variant="danger"
                loading={revoke.isPending}
                onClick={() =>
                  revoke.mutate(revoking.id, {
                    onSuccess: () => toast.success("Token revoked"),
                    onError: (error) => toast.error("Couldn't revoke token", errorMessage(error)),
                    onSettled: () => setRevoking(null),
                  })
                }
              >
                Revoke
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </Card>
  );
}

function CreateDialog({ onDone }: { onDone: () => void }) {
  const { create } = useProvisioningTokenMutations();
  const [name, setName] = useState("");
  const [copied, setCopied] = useState(false);

  if (create.data) {
    const { secret, base_url } = create.data;
    return (
      <DialogContent title="Copy your provisioning token" description="This is the only time it is shown. Paste it into your provider now.">
        <div className="space-y-4">
          <Field label="Base URL">
            <code className="block break-all rounded border border-line bg-sunken px-3 py-2 text-sm">{base_url}</code>
          </Field>
          <Field label="Token">
            <p aria-label="New provisioning token" className="break-all rounded border border-line bg-sunken px-3 py-2 font-mono text-sm text-ink">{secret}</p>
          </Field>
          <Button
            size="sm"
            onClick={() =>
              void navigator.clipboard?.writeText(secret).then(
                () => setCopied(true),
                () => toast.error("Couldn't copy", "Select the token and copy it instead."),
              )
            }
          >
            {copied ? <Check className="h-4 w-4" aria-hidden /> : <Copy className="h-4 w-4" aria-hidden />}
            {copied ? "Copied" : "Copy token"}
          </Button>
          <DialogFooter>
            <Button variant="primary" onClick={onDone}>Done</Button>
          </DialogFooter>
        </div>
      </DialogContent>
    );
  }
  const fieldError = create.error instanceof ApiError ? create.error.fieldErrors.find((e) => e.field === "name")?.message : undefined;
  const general = create.error && !fieldError ? errorMessage(create.error) : null;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (name.trim()) create.mutate(name.trim());
  };
  return (
    <DialogContent title="Create a provisioning token" description="Up to five per workspace, so you can rotate one without a gap.">
      <form noValidate onSubmit={submit} className="space-y-4">
        {general && <p role="alert" className="text-sm text-danger">{general}</p>}
        <Field label="Name" hint="Which provider or app uses it." error={fieldError}>
          <Input autoFocus maxLength={80} placeholder="Okta" value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <DialogFooter>
          <Button type="submit" variant="primary" loading={create.isPending} disabled={!name.trim()}>Create</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

const STATES: Record<string, { label: string; tone: "success" | "warning" | "danger" | "neutral" | "accent" }> = {
  member: { label: "Member", tone: "success" },
  suspended: { label: "Suspended", tone: "warning" },
  invited: { label: "Invited", tone: "accent" },
  invitation_expired: { label: "Invitation expired", tone: "warning" },
  not_invited: { label: "Invitation withdrawn", tone: "neutral" },
  deactivated: { label: "Deactivated", tone: "danger" },
};

function People() {
  const people = useProvisionedUsers();
  return (
    <Card>
      <CardHeader
        title="Provisioned people"
        description="New people get an invitation and join when they accept it with that address. People already in the workspace are matched by email."
      />
      {people.isPending ? (
        <div className="space-y-2 p-4" aria-busy><Skeleton className="h-12" /><Skeleton className="h-12" /></div>
      ) : people.isError ? (
        <ErrorState error={people.error} onRetry={() => void people.refetch()} />
      ) : people.data.items.length === 0 ? (
        <EmptyState icon={UserPlus} title="Nobody provisioned yet" description="People appear here once your provider starts sending them." />
      ) : (
        <>
          <ul aria-label="Provisioned people" className="divide-y divide-line">
            {people.data.items.map((person) => <PersonRow key={person.id} person={person} />)}
          </ul>
          {people.data.total > people.data.items.length && (
            <p className="border-t border-line px-4 py-3 text-sm text-ink-muted">
              Showing the {people.data.items.length} most recently changed of {people.data.total}.
            </p>
          )}
        </>
      )}
    </Card>
  );
}

function PersonRow({ person }: { person: ProvisionedUser }) {
  const state = STATES[person.state] ?? { label: person.state, tone: "neutral" as const };
  return (
    <li className="flex items-center gap-3 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="truncate text-base font-medium text-ink">{person.display_name ?? person.user_name}</div>
        <div className="truncate text-sm text-ink-muted">{person.email} · Updated {formatRelative(person.updated_at)}</div>
      </div>
      <Badge tone={state.tone}>{state.label}</Badge>
    </li>
  );
}
