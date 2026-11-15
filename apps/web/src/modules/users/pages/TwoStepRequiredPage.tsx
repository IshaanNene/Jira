import { useState } from "react";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router-dom";

import { Button } from "@/components/ui";
import { AuthLayout, FormError, switchOrganization, useMfaStatus } from "@/modules/auth";
import { useMyOrganizations } from "@/modules/organizations";
import { errorMessage } from "@/services/api";

import { TwoStepVerification } from "../components/TwoStepVerification";

/**
 * A workspace turned the person away because it requires two-step verification. Reached with
 * an account-level session, so set-up works here; then they continue into the workspace.
 */
export default function TwoStepRequiredPage() {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const workspaceId = params.get("workspace");
  const organizations = useMyOrganizations();
  const status = useMfaStatus();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!workspaceId) return <Navigate to="/onboarding" replace />;
  const name = organizations.data?.find((o) => o.id === workspaceId)?.name ?? "This workspace";

  const proceed = async () => {
    setBusy(true);
    setError(null);
    try {
      await switchOrganization(workspaceId);
      navigate("/", { replace: true });
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      title="Turn on two-step verification"
      subtitle={`${name} requires it of everyone who works in it. Set it up with an authenticator app to continue.`}
      footer={
        <Link to="/onboarding" className="font-medium text-ink-muted hover:text-ink">
          Choose another workspace
        </Link>
      }
    >
      <div className="space-y-4">
        <FormError message={error} />
        <TwoStepVerification />
        <Button variant="primary" className="w-full" disabled={!status.data?.totp_enabled} loading={busy} onClick={() => void proceed()}>
          Continue to {name}
        </Button>
      </div>
    </AuthLayout>
  );
}
