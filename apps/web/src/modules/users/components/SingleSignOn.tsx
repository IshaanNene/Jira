import { Building2, Link2 } from "lucide-react";
import { useEffect } from "react";
import { useSearchParams } from "react-router-dom";

import { Badge, Button, Card, CardHeader, ErrorState, Skeleton, toast } from "@/components/ui";
import { ssoErrorMessage, useLinkSso, useSsoLinks, useUnlinkSso, useWorkspaceSso } from "@/modules/auth";
import { errorMessage } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate, formatRelative } from "@/utils/format";

/**
 * Settings → Security → Single sign-on: which workspaces' identity providers can sign this
 * account in. Only shown when there is something to show or do.
 */
export function SingleSignOn() {
  const organizationId = useSession((s) => s.organization?.id);
  const links = useSsoLinks();
  const workspace = useWorkspaceSso(organizationId);
  const link = useLinkSso();
  const unlink = useUnlinkSso();
  const [params, setParams] = useSearchParams();

  // Back from the identity provider.
  useEffect(() => {
    const linked = params.get("sso") === "linked";
    const failure = ssoErrorMessage(params.get("sso_error"));
    if (!linked && !failure) return;
    if (linked) toast.success("Single sign-on linked", "You can now sign in to this workspace through its identity provider.");
    if (failure) toast.error("Couldn't link single sign-on", failure);
    setParams({}, { replace: true });
  }, [params, setParams]);

  const canLink = workspace.data?.available && !workspace.data.linked && organizationId;
  if (!canLink && (links.data?.length ?? 0) === 0) return null;

  return (
    <Card>
      <CardHeader
        title="Single sign-on"
        description="Workspaces whose identity provider can sign you in. Such a sign-in opens only that workspace."
        actions={
          canLink && (
            <Button size="sm" variant="primary" loading={link.isPending} onClick={() => link.mutate(organizationId, { onError: (e) => toast.error("Couldn't start linking", errorMessage(e)) })}>
              <Link2 className="h-4 w-4" aria-hidden /> Link this workspace
            </Button>
          )
        }
      />
      {canLink && (
        <p className="border-b border-line px-4 py-3 text-sm text-ink-muted">
          This workspace signs people in through its identity provider{workspace.data?.require_sso ? " and requires it" : ""}. Link your account
          to sign in with it, using the sign-in name <span className="font-mono text-ink">{workspace.data?.sign_in_name}</span>.
        </p>
      )}
      {links.isPending ? (
        <div className="p-4" aria-busy><Skeleton className="h-10" /></div>
      ) : links.isError ? (
        <ErrorState error={links.error} onRetry={() => void links.refetch()} />
      ) : (
        links.data.length > 0 && (
          <ul className="divide-y divide-line" aria-label="Linked workspaces">
            {links.data.map((l) => (
              <li key={l.organization_id} className="flex items-center gap-3 px-4 py-3">
                <Building2 className="h-5 w-5 shrink-0 text-ink-subtle" aria-hidden />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-ink">{l.sign_in_name}</span>
                    {l.organization_id === organizationId && <Badge tone="accent">This workspace</Badge>}
                  </div>
                  <p className="truncate text-sm text-ink-muted">
                    {l.email ?? "No email reported"} · linked {formatDate(l.created_at)} · {l.last_used_at ? `used ${formatRelative(l.last_used_at)}` : "never used"}
                  </p>
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`Unlink ${l.sign_in_name}`}
                  disabled={unlink.isPending}
                  onClick={() => unlink.mutate(l.organization_id, { onSuccess: () => toast.success("Single sign-on unlinked") })}
                >
                  Unlink
                </Button>
              </li>
            ))}
          </ul>
        )
      )}
    </Card>
  );
}
