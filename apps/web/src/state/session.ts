import { create } from "zustand";

import type { Account, OrgRole } from "@/services/api";

export type SessionStatus = "loading" | "anonymous" | "authenticated" | "unreachable";

export interface ActiveOrganization {
  id: string;
  role: OrgRole;
}

interface SessionState {
  status: SessionStatus;
  /** Held in memory only — never persisted to web storage. */
  accessToken: string | null;
  account: Account | null;
  organization: ActiveOrganization | null;
}

/**
 * Who is signed in and which organization they are acting in. This is session state,
 * not server data: lists and entities fetched from the API live in TanStack Query.
 */
export const useSession = create<SessionState>(() => ({
  status: "loading",
  accessToken: null,
  account: null,
  organization: null,
}));
