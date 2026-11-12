import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, type SessionList } from "@/services/api";

const sessionsKey = ["auth", "sessions"] as const;

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
