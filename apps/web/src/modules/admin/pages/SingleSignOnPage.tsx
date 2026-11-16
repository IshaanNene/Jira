import { Copy, Lock } from "lucide-react";
import { useState, type FormEvent } from "react";

import { Badge, Button, Card, CardHeader, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Field, Input, Page, Skeleton, Textarea, toast } from "@/components/ui";
import { can } from "@/modules/organizations";
import { ApiError, errorMessage, type SsoConnection } from "@/services/api";
import { useSession } from "@/state/session";

import { useSsoConnection, useSsoConnectionMutations } from "../api";

/** Settings → Single sign-on: connect the workspace to its identity provider (OpenID Connect or SAML 2.0). */
export default function SingleSignOnPage() {
  const role = useSession((s) => s.organization?.role);
  if (!can(role, "manageOrganization")) {
    return (
      <Page title="Single sign-on">
        <EmptyState icon={Lock} title="Admins only" description="Ask a workspace admin or owner to set up single sign-on." />
      </Page>
    );
  }
  return <SingleSignOn />;
}

function SingleSignOn() {
  const connection = useSsoConnection();
  return (
    <Page
      title="Single sign-on"
      description="Let members sign in through your identity provider (Okta, Microsoft Entra ID, Google Workspace, OneLogin, or any OpenID Connect or SAML 2.0 provider)."
      width="narrow"
    >
      {connection.isPending ? (
        <Card className="p-4"><Skeleton className="h-40" /></Card>
      ) : connection.isError ? (
        <Card><ErrorState error={connection.error} onRetry={() => void connection.refetch()} /></Card>
      ) : (
        <ConnectionForm key={connection.data?.updated_at ?? "new"} existing={connection.data} />
      )}
    </Page>
  );
}

function fieldError(error: unknown, field: string): string | undefined {
  return error instanceof ApiError ? error.fieldErrors.find((e) => e.field === field)?.message : undefined;
}

type Protocol = "oidc" | "saml";

function CopyRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="px-4 py-2">
      <p className="text-xs font-medium text-ink-muted">{label}</p>
      <div className="mt-1 flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded bg-sunken px-2 py-1 text-sm" aria-label={label}>{value}</code>
        <Button size="sm" onClick={() => void navigator.clipboard?.writeText(value).then(() => toast.success("Copied"))} aria-label={`Copy ${label.toLowerCase()}`}>
          <Copy className="h-4 w-4" aria-hidden />
        </Button>
      </div>
    </div>
  );
}

function ConnectionForm({ existing }: { existing: SsoConnection | null }) {
  const { save, remove } = useSsoConnectionMutations();
  const [protocol, setProtocol] = useState<Protocol>(existing?.protocol === "saml" ? "saml" : "oidc");
  const [metadata, setMetadata] = useState("");
  const savedSaml = existing?.protocol === "saml";
  const [form, setForm] = useState({
    sign_in_name: existing?.sign_in_name ?? "",
    issuer: existing?.protocol === "saml" ? "" : (existing?.issuer ?? ""),
    client_id: existing?.client_id ?? "",
    client_secret: "",
    enabled: existing?.enabled ?? true,
    require_sso: existing?.require_sso ?? false,
  });
  const [confirmRemove, setConfirmRemove] = useState(false);
  const set = (patch: Partial<typeof form>) => setForm((f) => ({ ...f, ...patch }));
  const general = save.error && !(save.error instanceof ApiError && save.error.fieldErrors.length > 0) ? errorMessage(save.error) : null;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const { sign_in_name, enabled, require_sso } = form;
    save.mutate(
      protocol === "saml"
        ? { protocol, sign_in_name, enabled, require_sso, saml_metadata: metadata.trim() || null }
        : { protocol, ...form, client_secret: form.client_secret.trim() || null },
      { onSuccess: () => toast.success("Single sign-on saved", "The provider's configuration was read successfully.") },
    );
  };

  return (
    <div className="space-y-5">
      {existing && (
        <Card>
          <CardHeader
            title="At your identity provider"
            description={
              existing.saml
                ? "Enter these in the SAML application you created for Meridian (or give it the metadata URL)."
                : "Register this redirect URI on the application you created for Meridian."
            }
            actions={existing.enabled ? <Badge tone="success">On</Badge> : <Badge>Off</Badge>}
          />
          {existing.saml ? (
            <div className="py-1">
              <CopyRow label="Entity ID (audience)" value={existing.saml.entity_id} />
              <CopyRow label="ACS URL" value={existing.saml.acs_url} />
              <CopyRow label="Metadata URL" value={existing.saml.metadata_url} />
              <p className="px-4 pb-2 text-xs text-ink-muted">
                Provider: <span className="font-mono">{existing.issuer}</span>. Send the email address as the name ID or an <span className="font-mono">email</span> attribute.
              </p>
            </div>
          ) : (
            <CopyRow label="Redirect URI" value={existing.redirect_uri} />
          )}
          <p className="border-t border-line px-4 py-3 text-sm text-ink-muted">
            {existing.linked_members} {existing.linked_members === 1 ? "member has" : "members have"} linked their account. Members link theirs under
            Settings → Security, then sign in with the name <span className="font-mono text-ink">{existing.sign_in_name}</span>.
          </p>
        </Card>
      )}
      <Card>
        <CardHeader
          title={existing ? "Provider" : "Connect a provider"}
          description={
            protocol === "saml"
              ? "Create a SAML 2.0 application at your provider, then paste its metadata."
              : "Create an OpenID Connect web application at your provider, then enter its details."
          }
        />
        <form noValidate onSubmit={submit} className="space-y-4 p-4">
          {general && <p role="alert" className="text-sm text-danger">{general}</p>}
          <fieldset>
            <legend className="mb-1.5 text-sm font-medium text-ink">Protocol</legend>
            <div role="radiogroup" aria-label="Protocol" className="inline-flex rounded border border-line-strong p-0.5">
              {(
                [
                  ["oidc", "OpenID Connect"],
                  ["saml", "SAML 2.0"],
                ] as const
              ).map(([value, label]) => (
                <button
                  key={value}
                  type="button"
                  role="radio"
                  aria-checked={protocol === value}
                  onClick={() => setProtocol(value)}
                  className={
                    protocol === value
                      ? "rounded-sm bg-accent px-3 py-1 text-sm font-medium text-accent-ink"
                      : "rounded-sm px-3 py-1 text-sm text-ink-muted hover:text-ink"
                  }
                >
                  {label}
                </button>
              ))}
            </div>
            {existing && (existing.protocol === "saml") !== (protocol === "saml") && (
              <p className="mt-1.5 text-xs text-warning">Switching protocol deletes every member's link; they link again afterwards.</p>
            )}
          </fieldset>
          <Field label="Sign-in name" hint="What members type on “Sign in with SSO”. Lower-case letters, digits and dashes." error={fieldError(save.error, "sign_in_name")}>
            <Input value={form.sign_in_name} autoCapitalize="none" spellCheck={false} onChange={(e) => set({ sign_in_name: e.target.value })} />
          </Field>
          {protocol === "saml" ? (
            <Field
              label="Identity provider metadata"
              hint={
                savedSaml
                  ? "Leave blank to keep the current metadata, or paste new XML (for example after the provider's certificate changes)."
                  : "The XML document from your provider's SAML app (“IdP metadata”). Its signing certificate is what responses are checked against."
              }
              error={fieldError(save.error, "saml_metadata")}
            >
              <Textarea
                value={metadata}
                rows={6}
                spellCheck={false}
                className="font-mono text-xs"
                placeholder={'<md:EntityDescriptor entityID="…">…</md:EntityDescriptor>'}
                onChange={(e) => setMetadata(e.target.value)}
              />
            </Field>
          ) : (
            <>
              <Field label="Issuer URL" hint="For example https://acme.okta.com or https://login.microsoftonline.com/<tenant>/v2.0" error={fieldError(save.error, "issuer")}>
                <Input value={form.issuer} type="url" spellCheck={false} onChange={(e) => set({ issuer: e.target.value })} />
              </Field>
              <Field label="Client ID" error={fieldError(save.error, "client_id")}>
                <Input value={form.client_id} spellCheck={false} onChange={(e) => set({ client_id: e.target.value })} />
              </Field>
              <Field
                label="Client secret"
                hint={existing?.protocol === "oidc" ? "Stored encrypted. Leave blank to keep the current one." : "Stored encrypted; never shown again."}
                error={fieldError(save.error, "client_secret")}
              >
                <Input value={form.client_secret} type="password" autoComplete="off" onChange={(e) => set({ client_secret: e.target.value })} />
              </Field>
            </>
          )}
          <label className="flex items-start gap-2 text-sm">
            <input type="checkbox" className="mt-0.5 h-4 w-4 accent-[hsl(var(--accent))]" checked={form.enabled} onChange={(e) => set({ enabled: e.target.checked })} />
            <span><span className="font-medium text-ink">Turned on</span><br /><span className="text-ink-muted">Members can sign in through the provider.</span></span>
          </label>
          <label className="flex items-start gap-2 text-sm">
            <input type="checkbox" className="mt-0.5 h-4 w-4 accent-[hsl(var(--accent))]" checked={form.require_sso} onChange={(e) => set({ require_sso: e.target.checked })} />
            <span>
              <span className="font-medium text-ink">Require single sign-on</span><br />
              <span className="text-ink-muted">Members other than owners can open this workspace only after signing in through the provider. Owners keep password access in case the provider fails.</span>
            </span>
          </label>
          <div className="flex justify-between gap-2">
            {existing ? <Button type="button" variant="ghost" onClick={() => setConfirmRemove(true)}>Remove</Button> : <span />}
            <Button type="submit" variant="primary" loading={save.isPending}>{existing ? "Save changes" : "Connect"}</Button>
          </div>
        </form>
      </Card>
      <Dialog open={confirmRemove} onOpenChange={setConfirmRemove}>
        {confirmRemove && (
          <DialogContent title="Remove single sign-on?" description="Members can no longer sign in through the provider, and every member's link is deleted. Recorded in the audit log.">
            <DialogFooter>
              <Button variant="ghost" onClick={() => setConfirmRemove(false)}>Cancel</Button>
              <Button
                variant="danger"
                loading={remove.isPending}
                onClick={() =>
                  remove.mutate(undefined, {
                    onSuccess: () => toast.success("Single sign-on removed"),
                    onError: (e) => toast.error("Couldn't remove single sign-on", errorMessage(e)),
                    onSettled: () => setConfirmRemove(false),
                  })
                }
              >
                Remove
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </div>
  );
}
