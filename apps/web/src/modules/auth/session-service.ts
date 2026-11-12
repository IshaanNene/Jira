import { api, ApiError, configureAuth, NetworkError, type OrgRole, type TokenResponse } from "@/services/api";
import { queryClient } from "@/services/query-client";
import { useSession } from "@/state/session";

const AUTH = "/api/v1/auth";
/** Remembers which organization to reopen. A preference only — it grants nothing. */
const LAST_ORG_KEY = "platform.lastOrganization";
/** Refresh this long before the access token expires. */
const REFRESH_MARGIN_MS = 60_000;

let refreshTimer: ReturnType<typeof setTimeout> | undefined;
let inflightRefresh: Promise<boolean> | null = null;
const channel = typeof BroadcastChannel === "undefined" ? null : new BroadcastChannel("platform.session");

function rememberOrganization(id: string | null): void {
  try {
    if (id) window.localStorage.setItem(LAST_ORG_KEY, id);
    else window.localStorage.removeItem(LAST_ORG_KEY);
  } catch {
    // Preference only; safe to lose.
  }
}

function rememberedOrganization(): string | null {
  try {
    return window.localStorage.getItem(LAST_ORG_KEY);
  } catch {
    return null;
  }
}

function apply(token: TokenResponse): void {
  const organization = token.organization ? { id: token.organization.id, role: token.organization.role as OrgRole } : null;
  useSession.setState({ status: "authenticated", accessToken: token.access_token, account: token.account, organization });
  if (organization) rememberOrganization(organization.id);
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => void refreshSession(), Math.max(token.expires_in * 1000 - REFRESH_MARGIN_MS, 5_000));
}

function clear(): void {
  clearTimeout(refreshTimer);
  useSession.setState({ status: "anonymous", accessToken: null, account: null, organization: null });
  // Nothing fetched for the previous user may be shown to the next one.
  queryClient.clear();
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function doRefresh(): Promise<boolean> {
  const organizationId = useSession.getState().organization?.id ?? rememberedOrganization() ?? undefined;
  for (let attempt = 0; ; attempt += 1) {
    try {
      apply(await api.post<TokenResponse>(`${AUTH}/token/refresh`, { organization_id: organizationId }, { auth: false }));
      return true;
    } catch (error) {
      // 409: another tab is rotating the same cookie right now; its new cookie will be
      // in the jar momentarily, so retry briefly instead of signing the user out.
      if (error instanceof ApiError && error.status === 409 && attempt < 3) {
        await sleep(200 * (attempt + 1));
        continue;
      }
      if (error instanceof ApiError && (error.status === 401 || error.status === 403)) {
        clear();
        return false;
      }
      throw error;
    }
  }
}

/** Single-flight: concurrent callers share one refresh request. */
export function refreshSession(): Promise<boolean> {
  inflightRefresh ??= doRefresh().finally(() => {
    inflightRefresh = null;
  });
  return inflightRefresh;
}

/** Called once at startup: restore the session from the refresh cookie, if there is one. */
export async function bootstrapSession(): Promise<void> {
  configureAuth({
    getToken: () => useSession.getState().accessToken,
    refresh: () => refreshSession().catch(() => false),
  });
  channel?.addEventListener("message", (event) => {
    if (event.data === "signed-out") clear();
    if (event.data === "signed-in" && useSession.getState().status === "anonymous") void refreshSession().catch(() => undefined);
  });
  try {
    await refreshSession();
  } catch (error) {
    useSession.setState({ status: error instanceof NetworkError || error instanceof ApiError ? "unreachable" : "anonymous" });
  }
}

export async function signIn(email: string, password: string): Promise<void> {
  apply(await api.post<TokenResponse>(`${AUTH}/login`, { email, password }, { auth: false }));
  channel?.postMessage("signed-in");
}

export async function signUp(input: { email: string; password: string; display_name: string }): Promise<void> {
  rememberOrganization(null);
  apply(await api.post<TokenResponse>(`${AUTH}/register`, input, { auth: false }));
  channel?.postMessage("signed-in");
}

export async function signOut(): Promise<void> {
  try {
    await api.post(`${AUTH}/logout`, undefined, { auth: false });
  } finally {
    // Local state is cleared even if the network call failed: the user asked to leave.
    clear();
    channel?.postMessage("signed-out");
  }
}

/**
 * Act inside `organizationId`. The server checks membership and decides the role; on
 * success every cached query is dropped because it belonged to the previous tenant.
 */
export async function switchOrganization(organizationId: string): Promise<void> {
  const token = await api.post<TokenResponse>(`${AUTH}/token/exchange`, { organization_id: organizationId });
  queryClient.clear();
  apply(token);
}

/** Leave the current organization scope without signing out (e.g. to pick another). */
export async function leaveOrganizationScope(): Promise<void> {
  rememberOrganization(null);
  useSession.setState({ organization: null });
  queryClient.clear();
  await refreshSession();
}
