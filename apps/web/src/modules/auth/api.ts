import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, type MfaStatus, type RecoveryCodes, type SessionList, type TotpSetup } from "@/services/api";

const sessionsKey = ["auth", "sessions"] as const;
const mfaKey = ["auth", "mfa"] as const;

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
