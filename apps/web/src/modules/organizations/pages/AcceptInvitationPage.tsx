import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { Button, Skeleton } from "@/components/ui";
import { AuthLayout, FormError, refreshSession, ResendButton, signOut, switchOrganization, twoStepRequiredPath, useSendVerification } from "@/modules/auth";
import { ApiError, errorMessage, type OrgRole } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate } from "@/utils/format";

import { useAcceptInvitation, useInvitationPreview } from "../api";
import { ROLE_LABEL } from "../permissions";

/** Landing page for an invitation link. Works signed out (preview) and signed in (accept). */
export default function AcceptInvitationPage() {
  const { token = "" } = useParams();
  const navigate = useNavigate();
  const status = useSession((s) => s.status);
  const account = useSession((s) => s.account);
  const preview = useInvitationPreview(token);
  const accept = useAcceptInvitation();
  const resend = useSendVerification();
  const [error, setError] = useState<string | null>(null);
  const [resent, setResent] = useState(false);

  if (preview.isPending || status === "loading") {
    return (
      <AuthLayout title="Checking your invitation">
        <div className="space-y-2" aria-busy>
          <Skeleton className="h-5 w-3/4" />
          <Skeleton className="h-10" />
        </div>
      </AuthLayout>
    );
  }

  if (preview.isError) {
    const gone = preview.error instanceof ApiError && preview.error.status === 404;
    return (
      <AuthLayout
        title={gone ? "This invitation isn't available" : "Couldn't load the invitation"}
        subtitle={gone ? "It may have been used already, revoked, or the link is incomplete. Ask the person who invited you for a new one." : errorMessage(preview.error)}
      >
        {gone ? (
          <Button asChild variant="primary" size="lg" className="w-full"><Link to="/">Go to {status === "authenticated" ? "your workspace" : "sign in"}</Link></Button>
        ) : (
          <Button size="lg" className="w-full" onClick={() => void preview.refetch()}>Try again</Button>
        )}
      </AuthLayout>
    );
  }

  const invitation = preview.data;
  const roleLabel = ROLE_LABEL[invitation.org_role as OrgRole] ?? invitation.org_role;
  const subtitle = (
    <>
      {invitation.invited_by_name ?? "A workspace admin"} invited <strong className="font-medium text-ink">{invitation.email}</strong> to join as {roleLabel}.
    </>
  );

  if (invitation.state === "expired") {
    return (
      <AuthLayout title={`Your invitation to ${invitation.organization_name} has expired`} subtitle={`It expired on ${formatDate(invitation.expires_at)}. Ask for a new invitation to join.`}>
        <Button asChild size="lg" className="w-full"><Link to="/">Continue</Link></Button>
      </AuthLayout>
    );
  }

  const next = `/invite/${token}`;
  const authQuery = `?next=${encodeURIComponent(next)}&email=${encodeURIComponent(invitation.email)}`;

  if (status !== "authenticated" || !account) {
    return (
      <AuthLayout title={`Join ${invitation.organization_name}`} subtitle={subtitle}>
        <div className="space-y-2">
          <Button asChild variant="primary" size="lg" className="w-full"><Link to={`/register${authQuery}`}>Create an account to accept</Link></Button>
          <Button asChild size="lg" className="w-full"><Link to={`/login${authQuery}`}>I already have an account</Link></Button>
        </div>
      </AuthLayout>
    );
  }

  const wrongAccount = account.email.toLowerCase() !== invitation.email.toLowerCase();
  const onAccept = async () => {
    setError(null);
    try {
      const result = await accept.mutateAsync(token).catch(async (e: unknown) => {
        // Verified in another tab since this token was issued: pick that up and try once more.
        if (e instanceof ApiError && e.code === "email_unverified" && (await refreshSession())) return accept.mutateAsync(token);
        throw e;
      });
      try {
        await switchOrganization(result.organization_id);
      } catch (e) {
        const required = twoStepRequiredPath(e, result.organization_id);
        if (required) return navigate(required, { replace: true });
        throw e;
      }
      navigate("/", { replace: true });
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <AuthLayout title={`Join ${invitation.organization_name}`} subtitle={subtitle}>
      <div className="space-y-3">
        <FormError message={error} />
        {wrongAccount ? (
          <>
            <p className="rounded border border-warning/30 bg-warning-soft px-3 py-2 text-sm text-warning">
              You are signed in as {account.email}. This invitation can only be accepted by {invitation.email}.
            </p>
            <Button size="lg" className="w-full" onClick={() => void signOut().then(() => navigate(`/login${authQuery}`))}>
              Sign in with a different account
            </Button>
          </>
        ) : (
          <>
            {!account.email_verified && (
              <p className="rounded border border-warning/30 bg-warning-soft px-3 py-2 text-sm text-ink">
                Verify {account.email} first: open the link we emailed you, then accept here.{" "}
                <ResendButton
                  sent={resent}
                  busy={resend.isPending}
                  onClick={() => resend.mutate(undefined, { onSuccess: () => setResent(true), onError: (e) => setError(errorMessage(e)) })}
                />
              </p>
            )}
            <Button variant="primary" size="lg" className="w-full" loading={accept.isPending} onClick={() => void onAccept()}>
              Accept invitation
            </Button>
          </>
        )}
      </div>
    </AuthLayout>
  );
}
