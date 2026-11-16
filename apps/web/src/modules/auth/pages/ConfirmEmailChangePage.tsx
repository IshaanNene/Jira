import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { Link, useSearchParams } from "react-router-dom";

import { Button, Skeleton } from "@/components/ui";
import { ApiError, errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

import { useConfirmEmailChange } from "../api";
import { AuthLayout } from "../components/AuthLayout";
import { refreshSession } from "../session-service";

/** Landing page for the link sent to a new address. Works signed in or out. */
export default function ConfirmEmailChangePage() {
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";
  const status = useSession((s) => s.status);
  const confirm = useConfirmEmailChange();
  const queryClient = useQueryClient();
  const started = useRef(false);

  useEffect(() => {
    if (started.current || !token) return;
    started.current = true;
    confirm.mutate(token, {
      // A signed-in tab picks up the new address straight away.
      onSuccess: () => {
        if (useSession.getState().status !== "authenticated") return;
        void refreshSession().then(() => queryClient.invalidateQueries());
      },
    });
  }, [token, confirm, queryClient]);

  if (!token || confirm.isError) {
    const fieldMessage = confirm.error instanceof ApiError ? confirm.error.fieldErrors[0]?.message : undefined;
    const message = !token ? "The link is incomplete. Open it again from the email." : (fieldMessage ?? errorMessage(confirm.error));
    return (
      <AuthLayout title="We couldn't change your email" subtitle={message}>
        <Button asChild variant="primary" size="lg" className="w-full">
          <Link to={status === "authenticated" ? "/settings/profile" : "/login"}>{status === "authenticated" ? "Back to your profile" : "Sign in"}</Link>
        </Button>
      </AuthLayout>
    );
  }

  if (!confirm.isSuccess) {
    return (
      <AuthLayout title="Changing your email">
        <div className="space-y-2" aria-busy><Skeleton className="h-5 w-3/4" /><Skeleton className="h-10" /></div>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout title="Email changed" subtitle="Sign in with your new address from now on. Your old address was told about the change.">
      <Button asChild variant="primary" size="lg" className="w-full">
        <Link to={status === "authenticated" ? "/settings/profile" : "/login"}>{status === "authenticated" ? "Continue" : "Sign in"}</Link>
      </Button>
    </AuthLayout>
  );
}
