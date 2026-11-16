import { useEffect, useState, type ReactNode } from "react";
import { Navigate, Outlet, useLocation } from "react-router-dom";

import { Button } from "@/components/ui";
import { bootstrapSession, safeNext, switchOrganization, twoStepRequiredPath } from "@/modules/auth";
import { LandingPage } from "@/modules/marketing";
import { useMyOrganizations } from "@/modules/organizations";
import { useSession } from "@/state/session";
import { PRODUCT_NAME } from "@/utils/config";

export function FullPageSpinner({ label = "Loading" }: { label?: string }) {
  return (
    <div role="status" aria-label={label} className="flex h-full items-center justify-center bg-canvas">
      <img src="/mark.svg" alt="" className="h-8 w-8 animate-pulse" />
    </div>
  );
}

function Unreachable() {
  const [retrying, setRetrying] = useState(false);
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 bg-canvas px-6 text-center">
      <img src="/mark.svg" alt="" className="h-8 w-8" />
      <h1 className="text-lg font-semibold text-ink">Can&rsquo;t reach {PRODUCT_NAME}</h1>
      <p className="max-w-sm text-sm text-ink-muted">The service isn&rsquo;t responding. Check your connection, then try again.</p>
      <Button
        variant="primary"
        loading={retrying}
        onClick={() => {
          setRetrying(true);
          useSession.setState({ status: "loading" });
          void bootstrapSession().finally(() => setRetrying(false));
        }}
      >
        Try again
      </Button>
    </div>
  );
}

/** Blocks rendering until the session is known; shows a retry screen if the API is down. */
export function SessionGate({ children }: { children: ReactNode }) {
  const status = useSession((s) => s.status);
  if (status === "loading") return <FullPageSpinner label="Restoring your session" />;
  if (status === "unreachable") return <Unreachable />;
  return <>{children}</>;
}

/** Routes for signed-in people. Anyone else is sent to sign in and brought back afterwards. */
export function RequireAuth() {
  const status = useSession((s) => s.status);
  const location = useLocation();
  if (status !== "authenticated") {
    const next = location.pathname + location.search;
    // Signed-out visitors to the root see the product's public home page.
    if (next === "/") return <LandingPage />;
    return <Navigate to={`/login?next=${encodeURIComponent(next)}`} replace />;
  }
  return <Outlet />;
}

/** Sign-in and sign-up are pointless once signed in. */
export function RedirectIfAuthenticated() {
  const status = useSession((s) => s.status);
  const location = useLocation();
  if (status === "authenticated") return <Navigate to={safeNext(new URLSearchParams(location.search).get("next"))} replace />;
  return <Outlet />;
}

/**
 * Routes that need an active workspace. With none selected: open the only workspace
 * automatically, otherwise send the person to choose or create one.
 */
export function RequireOrganization() {
  const organization = useSession((s) => s.organization);
  const organizations = useMyOrganizations();
  const [failed, setFailed] = useState<string | false>(false);
  const only = !organization && organizations.data?.length === 1 ? organizations.data[0] : undefined;

  useEffect(() => {
    if (only && !failed) switchOrganization(only.id).catch((error) => setFailed(twoStepRequiredPath(error, only.id) ?? "/onboarding"));
  }, [only, failed]);

  if (organization) return <Outlet />;
  if (organizations.isPending || (only && !failed)) return <FullPageSpinner label="Opening your workspace" />;
  return <Navigate to={failed || "/onboarding"} replace />;
}
