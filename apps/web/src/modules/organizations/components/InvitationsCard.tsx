import { Check, Copy, MailPlus } from "lucide-react";
import { useState } from "react";

import { Badge, Button, Card, CardHeader, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Field, Input, Skeleton, toast } from "@/components/ui";
import { ApiError, errorMessage, type Invitation } from "@/services/api";
import { formatRelative } from "@/utils/format";

import { useInvitations, useResendInvitation, useRevokeInvitation } from "../api";
import { RoleBadge } from "./RoleBadge";

/** Open invitations. Rendered only for people allowed to invite. */
export function InvitationsCard() {
  const invitations = useInvitations(true);
  const revoke = useRevokeInvitation();
  const resend = useResendInvitation();
  // A new link that could not be emailed (no mail server): shown once to pass on.
  const [unsent, setUnsent] = useState<Invitation | null>(null);
  const [copied, setCopied] = useState(false);

  const sendAgain = (invitation: Invitation) =>
    resend.mutate(invitation.id, {
      onSuccess: (renewed) => {
        if (renewed.last_sent_at && renewed.last_sent_at !== invitation.last_sent_at) toast.success("Invitation sent again", `A new link went to ${renewed.email}; the old one no longer works.`);
        else setUnsent(renewed);
      },
      onError: (error) =>
        toast.error(
          "Couldn't send the invitation",
          error instanceof ApiError && error.status === 429 ? "It was emailed a moment ago. Try again in a minute." : errorMessage(error),
        ),
    });

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
                  {invitation.last_sent_at ? ` · emailed ${formatRelative(invitation.last_sent_at)}` : ""}
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
                aria-label={`Send the invitation for ${invitation.email} again`}
                loading={resend.isPending && resend.variables === invitation.id}
                onClick={() => sendAgain(invitation)}
              >
                {invitation.state === "expired" ? "Renew" : "Resend"}
              </Button>
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
      <Dialog
        open={unsent !== null}
        onOpenChange={(open) => {
          if (!open) {
            setUnsent(null);
            setCopied(false);
          }
        }}
      >
        {unsent && (
          <DialogContent title="New invitation link" description={`Email isn't set up here, so share this link with ${unsent.email} yourself. The old link no longer works.`}>
            <Field label="Invitation link" hint="It is shown only once and works only for the invited email address.">
              <Input readOnly value={unsent.accept_url ?? ""} onFocus={(e) => e.currentTarget.select()} className="font-mono text-sm" />
            </Field>
            <DialogFooter>
              <Button
                onClick={() =>
                  void navigator.clipboard.writeText(unsent.accept_url ?? "").then(() => setCopied(true))
                }
              >
                {copied ? <Check className="h-3.5 w-3.5 text-success" aria-hidden /> : <Copy className="h-3.5 w-3.5" aria-hidden />}
                {copied ? "Copied" : "Copy link"}
              </Button>
              <Button variant="primary" onClick={() => setUnsent(null)}>
                Done
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </Card>
  );
}
