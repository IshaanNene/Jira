import { Link, useSearchParams } from "react-router-dom";

import { Button, Skeleton } from "@/components/ui";
import { AuthLayout, FormError } from "@/modules/auth";
import { ApiError, errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

import { useUnsubscribe, useUnsubscribeLink } from "../api";

/** Landing page for the unsubscribe link in notification emails. Works signed out. */
export default function UnsubscribePage() {
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";
  const link = useUnsubscribeLink(token);
  const unsubscribe = useUnsubscribe(token);
  const signedIn = useSession((s) => s.status === "authenticated");
  const settings = (
    <Button asChild variant="ghost" className="w-full">
      <Link to={signedIn ? "/settings/notifications" : "/login"}>{signedIn ? "Choose which emails you get" : "Sign in to choose which emails you get"}</Link>
    </Button>
  );

  if (!token || link.isError) {
    const gone = !token || (link.error instanceof ApiError && link.error.status === 404);
    return (
      <AuthLayout
        title={gone ? "This unsubscribe link doesn't work" : "Couldn't check this link"}
        subtitle={gone ? "It may be incomplete or older than 90 days. You can still turn emails off in your notification settings." : errorMessage(link.error)}
      >
        {settings}
      </AuthLayout>
    );
  }

  if (link.isPending) {
    return (
      <AuthLayout title="Checking your link">
        <div className="space-y-2" aria-busy>
          <Skeleton className="h-10" />
          <Skeleton className="h-10" />
        </div>
      </AuthLayout>
    );
  }

  if (unsubscribe.isSuccess) {
    const all = unsubscribe.variables;
    return (
      <AuthLayout
        title="You're unsubscribed"
        subtitle={all ? "You won't get notification emails from this workspace any more." : `You won't get emails about ${link.data.kind_label} any more.`}
      >
        <p className="text-sm text-ink-muted">You'll still see notifications in the app.</p>
        {settings}
      </AuthLayout>
    );
  }

  return (
    <AuthLayout title={`Stop emails about ${link.data.kind_label}?`} subtitle="Choose what to turn off for this workspace. You'll still see notifications in the app.">
      {unsubscribe.isError && <FormError message={errorMessage(unsubscribe.error)} />}
      <div className="space-y-2">
        <Button variant="primary" size="lg" className="w-full" loading={unsubscribe.isPending && !unsubscribe.variables} onClick={() => unsubscribe.mutate(false)}>
          Stop emails about {link.data.kind_label}
        </Button>
        <Button size="lg" className="w-full" loading={unsubscribe.isPending && unsubscribe.variables} onClick={() => unsubscribe.mutate(true)}>
          Turn off all notification emails
        </Button>
      </div>
      {settings}
    </AuthLayout>
  );
}
