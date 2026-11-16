import { KeyRound, Lock, ShieldAlert, ShieldCheck, Trash2 } from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";

import { Badge, Button, Card, CardHeader, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Page, Skeleton, toast } from "@/components/ui";
import { useWebhooks } from "@/modules/integrations";
import { can, ROLE_LABEL, useCurrentOrganization, useUserDirectory } from "@/modules/organizations";
import { useProjects } from "@/modules/projects";
import { errorMessage, type OrgRole, type WorkspaceApiToken } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate, formatRelative } from "@/utils/format";

import { useMemberTwoStep, useRevokeWorkspaceApiToken, useWorkspaceApiTokens } from "../api";

/** Settings → Admin console: the workspace's people, sign-in security and integrations at a glance. */
export default function AdminConsolePage() {
  const role = useSession((s) => s.organization?.role);
  if (!can(role, "manageOrganization")) {
    return (
      <Page title="Admin console">
        <EmptyState icon={Lock} title="Admins only" description="Ask a workspace admin or owner for anything you need here." />
      </Page>
    );
  }
  return (
    <Page title="Admin console" description="Who is in this workspace, how well their sign-ins are protected, and what is connected to it.">
      <div className="space-y-5">
        <Overview />
        <TwoStepAdoption />
        <ApiTokens />
      </div>
    </Page>
  );
}

const ROLES: OrgRole[] = ["owner", "admin", "member", "guest"];

function Stat({ label, value, children }: { label: string; value: ReactNode; children?: ReactNode }) {
  return (
    <div className="rounded-md border border-line p-3">
      <p className="text-sm text-ink-muted">{label}</p>
      <p className="mt-1 text-2xl font-semibold tabular-nums text-ink">{value}</p>
      {children && <div className="mt-1 text-sm text-ink-muted">{children}</div>}
    </div>
  );
}

function Overview() {
  const directory = useUserDirectory();
  const projects = useProjects();
  const organization = useCurrentOrganization();
  const hooks = useWebhooks();
  const active = directory.members.filter((m) => m.status === "active");
  const byRole = ROLES.map((r) => [r, active.filter((m) => m.org_role === r).length] as const).filter(([, n]) => n > 0);
  const failing = (hooks.data ?? []).filter((h) => !h.enabled && h.disabled_reason).length;

  return (
    <Card>
      <CardHeader title="Overview" />
      <div className="grid gap-3 p-4 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label="Active members" value={directory.isPending ? "…" : active.length}>
          {byRole.map(([r, n]) => `${n} ${ROLE_LABEL[r].toLowerCase()}${n === 1 ? "" : "s"}`).join(" · ")}
        </Stat>
        <Stat label="Projects" value={projects.data ? projects.data.length : "…"}>
          <Link className="text-accent hover:underline" to="/projects">See all</Link>
        </Stat>
        <Stat label="Two-step verification" value={organization.data ? (organization.data.require_two_step ? "Required" : "Optional") : "…"}>
          <Link className="text-accent hover:underline" to="/settings/workspace">Change</Link>
        </Stat>
        <Stat label="Webhooks" value={hooks.data ? hooks.data.length : "…"}>
          {failing > 0 ? (
            <Link className="text-danger hover:underline" to="/settings/webhooks">{failing} turned off after failures</Link>
          ) : (
            <Link className="text-accent hover:underline" to="/settings/webhooks">Manage</Link>
          )}
        </Stat>
      </div>
    </Card>
  );
}

function TwoStepAdoption() {
  const directory = useUserDirectory();
  const active = useMemo(() => directory.members.filter((m) => m.status === "active"), [directory.members]);
  const ids = useMemo(() => active.map((m) => m.user_id).sort(), [active]);
  const status = useMemberTwoStep(ids);
  const without = status.data ? active.filter((m) => status.data.get(m.user_id) === false) : [];
  const withCount = status.data ? [...status.data.values()].filter(Boolean).length : 0;

  return (
    <Card>
      <CardHeader
        title="Two-step verification"
        description="Members without it can be locked out of their account by anyone who learns their password."
        actions={status.data && <Badge tone={without.length === 0 ? "success" : "warning"}>{withCount} of {status.data.size} on</Badge>}
      />
      {directory.isPending || (ids.length > 0 && status.isPending) ? (
        <div className="p-4" aria-busy><Skeleton className="h-10" /></div>
      ) : status.isError ? (
        <ErrorState error={status.error} onRetry={() => void status.refetch()} />
      ) : without.length === 0 ? (
        <EmptyState icon={ShieldCheck} title="Everyone is covered" description="Every active member has two-step verification on." />
      ) : (
        <>
          <ul className="divide-y divide-line" aria-label="Members without two-step verification">
            {without.map((m) => (
              <li key={m.user_id} className="flex items-center gap-3 px-4 py-2">
                <ShieldAlert className="h-4 w-4 text-warning" aria-hidden />
                <span className="min-w-0 flex-1 truncate text-ink">{m.display_name}</span>
                <span className="text-sm text-ink-muted">{ROLE_LABEL[m.org_role as OrgRole] ?? m.org_role}</span>
              </li>
            ))}
          </ul>
          <p className="border-t border-line px-4 py-3 text-sm text-ink-muted">
            Ask them to turn it on under Settings → Security, or{" "}
            <Link className="text-accent hover:underline" to="/settings/workspace">require it for the workspace</Link>. Admins cannot reset someone
            else's two-step verification: an account can belong to several workspaces.
          </p>
        </>
      )}
    </Card>
  );
}

function ApiTokens() {
  const tokens = useWorkspaceApiTokens();
  const directory = useUserDirectory();
  const revoke = useRevokeWorkspaceApiToken();
  const [confirming, setConfirming] = useState<WorkspaceApiToken | null>(null);
  const owner = (id: string) => directory.byId.get(id)?.display_name ?? "A former member";

  return (
    <Card>
      <CardHeader title="API tokens" description="Every active token that can act in this workspace. Revoking one stops it working immediately." />
      {tokens.isPending ? (
        <div className="p-4" aria-busy><Skeleton className="h-10" /></div>
      ) : tokens.isError ? (
        <ErrorState error={tokens.error} onRetry={() => void tokens.refetch()} />
      ) : tokens.data.length === 0 ? (
        <EmptyState icon={KeyRound} title="No API tokens" description="Members create tokens for scripts and integrations under Settings → Security." />
      ) : (
        <ul className="divide-y divide-line" aria-label="API tokens">
          {tokens.data.map((t) => (
            <li key={t.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <div className="min-w-0 flex-1">
                <p className="font-medium text-ink">{t.name} <span className="font-mono text-xs text-ink-muted">{t.hint}…</span></p>
                <p className="text-sm text-ink-muted">
                  {owner(t.owner_id)} · expires {formatDate(t.expires_at)} · {t.last_used_at ? `last used ${formatRelative(t.last_used_at)}` : "never used"}
                </p>
              </div>
              <Button size="sm" variant="ghost" aria-label={`Revoke ${t.name}`} onClick={() => setConfirming(t)}>
                <Trash2 className="h-4 w-4" aria-hidden /> Revoke
              </Button>
            </li>
          ))}
        </ul>
      )}
      <Dialog open={confirming !== null} onOpenChange={(open) => !open && setConfirming(null)}>
        {confirming && (
          <DialogContent title={`Revoke “${confirming.name}”?`} description={`${owner(confirming.owner_id)}'s scripts using it will stop working. This is recorded in the audit log.`}>
            <DialogFooter>
              <Button variant="ghost" onClick={() => setConfirming(null)}>Cancel</Button>
              <Button
                variant="danger"
                disabled={revoke.isPending}
                onClick={() =>
                  revoke.mutate(confirming.id, {
                    onSuccess: () => {
                      toast.success("Token revoked");
                      setConfirming(null);
                    },
                    onError: (e) => toast.error("Couldn't revoke the token", errorMessage(e)),
                  })
                }
              >
                Revoke token
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </Card>
  );
}
