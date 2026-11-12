import { MailPlus } from "lucide-react";

import { Badge, Button, Card, CardHeader, EmptyState, ErrorState, Skeleton, toast } from "@/components/ui";
import { errorMessage } from "@/services/api";
import { formatRelative } from "@/utils/format";

import { useInvitations, useRevokeInvitation } from "../api";
import { RoleBadge } from "./RoleBadge";

/** Open invitations. Rendered only for people allowed to invite. */
export function InvitationsCard() {
  const invitations = useInvitations(true);
  const revoke = useRevokeInvitation();

  return (
    <Card>
      <CardHeader title="Pending invitations" description="People who have been invited but have not joined yet." />
      {invitations.isPending ? (
        <div className="space-y-2 p-4" aria-busy>
          <Skeleton className="h-9" />
          <Skeleton className="h-9" />
        </div>
      ) : invitations.isError ? (
        <ErrorState error={invitations.error} onRetry={() => void invitations.refetch()} />
      ) : invitations.data.length === 0 ? (
        <EmptyState icon={MailPlus} title="No pending invitations" description="Invite teammates by email. Each invitation is a single-use link that expires after 7 days." />
      ) : (
        <ul className="divide-y divide-line">
          {invitations.data.map((invitation) => (
            <li key={invitation.id} className="flex items-center gap-3 px-4 py-2.5">
              <div className="min-w-0 flex-1">
                <div className="truncate text-base font-medium text-ink">{invitation.email}</div>
                <div className="truncate text-sm text-ink-muted">
                  Invited {formatRelative(invitation.created_at)}
                  {invitation.invited_by_name ? ` by ${invitation.invited_by_name}` : ""}
                </div>
              </div>
              <RoleBadge role={invitation.org_role} />
              {invitation.state === "expired" ? (
                <Badge tone="warning">Expired</Badge>
              ) : (
                <span className="hidden text-sm text-ink-subtle sm:inline">Expires {formatRelative(invitation.expires_at)}</span>
              )}
              <Button
                size="sm"
                variant="ghost"
                aria-label={`Revoke invitation for ${invitation.email}`}
                onClick={() =>
                  revoke.mutate(invitation.id, {
                    onSuccess: () => toast.success("Invitation revoked"),
                    onError: (error) => toast.error("Couldn't revoke invitation", errorMessage(error)),
                  })
                }
              >
                Revoke
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
