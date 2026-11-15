import { lazy } from "react";

export {
  useApiTokens,
  useConfirmTotp,
  useCreateApiToken,
  useRevokeApiToken,
  useDisableTotp, useMfaStatus, useSendVerification, useRegenerateRecoveryCodes, useRevokeSession, useSessions, useStartTotpSetup } from "./api";
export { AuthLayout } from "./components/AuthLayout";
export { ResendButton, VerifyEmailBanner } from "./components/VerifyEmailBanner";
export { FormError } from "./components/FormError";
export { applyServerErrors } from "./form-errors";
export { safeNext, twoStepRequiredPath } from "./navigation";
export { bootstrapSession, completeSignIn, leaveOrganizationScope, refreshSession, signIn, signOut, signUp, switchOrganization } from "./session-service";

export const LoginPage = lazy(() => import("./pages/LoginPage"));
export const RegisterPage = lazy(() => import("./pages/RegisterPage"));
export const ForgotPasswordPage = lazy(() => import("./pages/ForgotPasswordPage"));
export const ResetPasswordPage = lazy(() => import("./pages/ResetPasswordPage"));
export const VerifyEmailPage = lazy(() => import("./pages/VerifyEmailPage"));
