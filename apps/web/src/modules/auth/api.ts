import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  api,
  type ApiTokenList,
  type CreateApiToken,
  type CreatedApiToken,
  type MfaStatus,
  type Passkey,
  type PasskeyList,
  type PasskeyOptions,
  type RecoveryCodes, type SessionList, type TotpSetup } from "@/services/api";

import { createPasskey, type CreationOptionsJson } from "./webauthn";

const sessionsKey = ["auth", "sessions"] as const;
const mfaKey = ["auth", "mfa"] as const;
const passkeysKey = ["auth", "passkeys"] as const;
/** Tokens belong to one organization, so the list is keyed by it. */
const apiTokensKey = (organizationId: string) => ["auth", "api-tokens", organizationId] as const;

export function useSessions() {
  return useQuery({ queryKey: sessionsKey, queryFn: () => api.get<SessionList>("/api/v1/auth/sessions") });
}

export function useRevokeSession() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.delete(`/api/v1/auth/sessions/${id}`),
    onSettled: () => queryClient.invalidateQueries({ queryKey: sessionsKey }),
  });
}

export function useMfaStatus() {
  return useQuery({ queryKey: mfaKey, queryFn: () => api.get<MfaStatus>("/api/v1/auth/mfa") });
}

/** Turning two-step verification on or off ends the other sessions, so both lists change. */
function useRefreshSecurity() {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: mfaKey });
    void queryClient.invalidateQueries({ queryKey: sessionsKey });
  };
}

export function useStartTotpSetup() {
  return useMutation({ mutationFn: (password: string) => api.post<TotpSetup>("/api/v1/auth/mfa/totp/setup", { password }) });
}

export function useConfirmTotp() {
  const refresh = useRefreshSecurity();
  return useMutation({
    mutationFn: (code: string) => api.post<RecoveryCodes>("/api/v1/auth/mfa/totp/confirm", { code }),
    onSuccess: refresh,
  });
}

export function useRegenerateRecoveryCodes() {
  const refresh = useRefreshSecurity();
  return useMutation({
    mutationFn: (input: { password: string; code: string }) => api.post<RecoveryCodes>("/api/v1/auth/mfa/recovery-codes", input),
    onSuccess: refresh,
  });
}

export function useDisableTotp() {
  const refresh = useRefreshSecurity();
  return useMutation({
    mutationFn: (input: { password: string; code: string }) => api.post("/api/v1/auth/mfa/totp/disable", input),
    onSuccess: refresh,
  });
}

export function usePasskeys() {
  return useQuery({ queryKey: passkeysKey, queryFn: () => api.get<PasskeyList>("/api/v1/auth/passkeys"), select: (d) => d.items });
}

/** Asks for the password, then has the browser make a passkey, then saves it. */
export function useAddPasskey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ password, name }: { password: string; name: string }) => {
      const options = await api.post<PasskeyOptions>("/api/v1/auth/passkeys/options", { password });
      const response = await createPasskey(options.public_key as unknown as CreationOptionsJson);
      return api.post<Passkey>("/api/v1/auth/passkeys", { name, ...response });
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: passkeysKey }),
  });
}

export function useRenamePasskey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) => api.patch<Passkey>(`/api/v1/auth/passkeys/${id}`, { name }),
    onSettled: () => queryClient.invalidateQueries({ queryKey: passkeysKey }),
  });
}

export function useRemovePasskey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, password }: { id: string; password: string }) => api.post(`/api/v1/auth/passkeys/${id}/remove`, { password }),
    onSettled: () => queryClient.invalidateQueries({ queryKey: passkeysKey }),
  });
}

export function useApiTokens(organizationId: string) {
  return useQuery({ queryKey: apiTokensKey(organizationId), queryFn: () => api.get<ApiTokenList>("/api/v1/auth/api-tokens") });
}

export function useCreateApiToken(organizationId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateApiToken) => api.post<CreatedApiToken>("/api/v1/auth/api-tokens", input),
    onSettled: () => queryClient.invalidateQueries({ queryKey: apiTokensKey(organizationId) }),
  });
}

export function useRevokeApiToken(organizationId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.delete(`/api/v1/auth/api-tokens/${id}`),
    onSettled: () => queryClient.invalidateQueries({ queryKey: apiTokensKey(organizationId) }),
  });
}

/** Email the signed-in person a new link to verify their address (202), unless it already is (204). */
export function useSendVerification() {
  return useMutation({ mutationFn: () => api.post("/api/v1/auth/email/verification") });
}

export function useVerifyEmail() {
  return useMutation({ mutationFn: (token: string) => api.post("/api/v1/auth/email/verify", { token }, { auth: false }) });
}

/** Emails a link to the new address; nothing changes until it is opened. */
export function useChangeEmail() {
  return useMutation({
    mutationFn: (input: { new_email: string; password: string }) => api.post("/api/v1/auth/email/change", input),
  });
}

export function useConfirmEmailChange() {
  return useMutation({ mutationFn: (token: string) => api.post("/api/v1/auth/email/change/confirm", { token }, { auth: false }) });
}

export function useForgotPassword() {
  return useMutation({ mutationFn: (email: string) => api.post("/api/v1/auth/password/forgot", { email }, { auth: false }) });
}

export function useResetPassword() {
  return useMutation({
    mutationFn: (input: { token: string; password: string }) => api.post("/api/v1/auth/password/reset", input, { auth: false }),
  });
}
