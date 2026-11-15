import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";

import { Button, Skeleton, toast } from "@/components/ui";
import { ApiError, errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

import { useSendVerification, useVerifyEmail } from "../api";
import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";
import { refreshSession } from "../session-service";

/** Landing page for the link in a verification email. Works signed in or out. */
export default function VerifyEmailPage() {
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";
  const status = useSession((s) => s.status);
  const verify = useVerifyEmail();
  const resend = useSendVerification();
  const started = useRef(false);
  const [resent, setResent] = useState(false);

  useEffect(() => {
    if (started.current || !token) return;
    started.current = true;
    verify.mutate(token, {
      // A signed-in tab picks up the verified address straight away.
      onSuccess: () => void (useSession.getState().status === "authenticated" && refreshSession()),
    });
  }, [token, verify]);

  if (!token || verify.isError) {
    const fieldMessage = verify.error instanceof ApiError ? verify.error.fieldErrors[0]?.message : undefined;
    const message = !token ? "The link is incomplete. Open it again from the email." : (fieldMessage ?? errorMessage(verify.error));
    return (
      <AuthLayout title="We couldn't verify your email" subtitle={message}>
        <div className="space-y-2">
          {status === "authenticated" ? (
            <Button
              variant="primary"
              size="lg"
              className="w-full"
              loading={resend.isPending}
              disabled={resent}
              onClick={() =>
                resend.mutate(undefined, {
                  onSuccess: () => {
                    setResent(true);
                    toast.success("Link sent", "Check your inbox.");
                  },
                })
              }
            >
              {resent ? "Link sent" : "Send a new link"}
            </Button>
          ) : (
            <Button asChild variant="primary" size="lg" className="w-full"><Link to="/login">Sign in to get a new link</Link></Button>
          )}
          <FormError message={resend.error ? errorMessage(resend.error) : null} />
        </div>
      </AuthLayout>
    );
  }

  if (!verify.isSuccess) {
    return (
      <AuthLayout title="Verifying your email">
        <div className="space-y-2" aria-busy><Skeleton className="h-5 w-3/4" /><Skeleton className="h-10" /></div>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout title="Email verified" subtitle="Thanks. You can now accept invitations sent to this address.">
      <Button asChild variant="primary" size="lg" className="w-full">
        <Link to="/">{status === "authenticated" ? "Continue" : "Sign in"}</Link>
      </Button>
    </AuthLayout>
  );
}
