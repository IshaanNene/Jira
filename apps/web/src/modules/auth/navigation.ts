/**
 * Where to go after signing in. Only same-app absolute paths are honoured, so a crafted
 * `?next=https://evil.example` (or `//evil.example`) cannot turn login into an open redirect.
 */
export function safeNext(next: string | null | undefined): string {
  if (!next || !next.startsWith("/") || next.startsWith("//") || next.includes("\\")) return "/";
  return next;
}
