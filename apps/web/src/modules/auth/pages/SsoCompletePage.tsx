import { useEffect, useRef, useState } from "react";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router-dom";

import { errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";
import { switchOrganization } from "../session-service";

/**
 * Back from the identity provider, signed in: the session (restored from the cookie the
 * callback set) can enter only that workspace, so open it.
 */
export default function SsoCompletePage() {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const status = useSession((s) => s.status);
  const workspace = params.get("workspace");
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false);

  useEffect(() => {
    if (status !== "authenticated" || !workspace || started.current) return;
    started.current = true;
    switchOrganization(workspace)
      .then(() => navigate("/", { replace: true }))
      .catch((e: unknown) => setError(errorMessage(e)));
  }, [status, workspace, navigate]);

  if (!workspace || status === "anonymous") return <Navigate to="/login?sso_error=expired" replace />;
  return (
    <AuthLayout title="Signing you in" subtitle="Opening your workspace…" footer={<Link to="/login" className="font-medium text-accent hover:underline">Back to sign in</Link>}>
      <FormError message={error} />
    </AuthLayout>
  );
}
