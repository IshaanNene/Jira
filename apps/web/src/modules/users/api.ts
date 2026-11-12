import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, type Profile, type UpdateProfileRequest } from "@/services/api";
import { useSession } from "@/state/session";

const profileKey = (userId: string | undefined) => ["users", "me", userId] as const;

export function useMyProfile() {
  const userId = useSession((s) => s.account?.id);
  return useQuery({ queryKey: profileKey(userId), queryFn: () => api.get<Profile>("/api/v1/users/me"), enabled: Boolean(userId) });
}

export function useUpdateProfile() {
  const queryClient = useQueryClient();
  const userId = useSession((s) => s.account?.id);
  return useMutation({
    mutationFn: (input: UpdateProfileRequest) => api.patch<Profile>("/api/v1/users/me", input),
    onSuccess: (profile) => {
      queryClient.setQueryData(profileKey(userId), profile);
      // Names appear in member lists across the app.
      void queryClient.invalidateQueries({ queryKey: ["org"] });
    },
  });
}
