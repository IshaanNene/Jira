import { ApiError } from "@/services/api";

/**
 * Where to go after signing in. Only same-app absolute paths are honoured, so a crafted
 * `?next=https://evil.example` (or `//evil.example`) cannot turn login into an open redirect.
 */
export function safeNext(next: string | null | undefined): string {
  if (!next || !next.startsWith("/") || next.startsWith("//") || next.includes("\\")) return "/";
  return next;
}

/**
 * Where to send someone a workspace turned away because it requires two-step verification
 * they have not turned on; `null` for any other failure.
 */
export function twoStepRequiredPath(error: unknown, organizationId: string): string | null {
  if (error instanceof ApiError && error.code === "two_step_required") return `/two-step-required?workspace=${encodeURIComponent(organizationId)}`;
  return null;
}
