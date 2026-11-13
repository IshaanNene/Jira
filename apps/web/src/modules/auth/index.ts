import { lazy } from "react";

export { useConfirmTotp, useDisableTotp, useMfaStatus, useRegenerateRecoveryCodes, useRevokeSession, useSessions, useStartTotpSetup } from "./api";
export { AuthLayout } from "./components/AuthLayout";
export { FormError } from "./components/FormError";
export { applyServerErrors } from "./form-errors";
export { safeNext } from "./navigation";
export { bootstrapSession, completeSignIn, leaveOrganizationScope, refreshSession, signIn, signOut, signUp, switchOrganization } from "./session-service";

export const LoginPage = lazy(() => import("./pages/LoginPage"));
export const RegisterPage = lazy(() => import("./pages/RegisterPage"));
