import type { QueryClient, QueryKey } from "@tanstack/react-query";

import type { Hint } from "@/services/realtime";

const DELAY = 250;

/**
 * Collects query keys to refetch and flushes them together, a moment later. While the
 * person's own change is in flight the flush waits, so a refetch never overwrites an
 * optimistic update with the state from before it.
 */
export class RefreshBatch {
  private keys = new Map<string, QueryKey>();
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly qc: QueryClient) {}

  add(...keys: QueryKey[]): void {
    for (const key of keys) this.keys.set(JSON.stringify(key), key);
    this.schedule();
  }

  private schedule(): void {
    if (this.timer) return;
    this.timer = setTimeout(() => this.flush(), DELAY);
  }

  private flush(): void {
    this.timer = undefined;
    if (this.qc.isMutating() > 0) {
      this.schedule();
      return;
    }
    const keys = [...this.keys.values()];
    this.keys.clear();
    for (const queryKey of keys) void this.qc.invalidateQueries({ queryKey });
  }

  dispose(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
    this.keys.clear();
  }
}

export interface ProjectKeys {
  /** Issue lists, boards, backlog and reports of the project. */
  lists: QueryKey;
  issue: (key: string) => QueryKey;
  project: QueryKey;
}

/** Which cached queries a hint on a project channel makes stale. */
export function staleFor(hint: Hint, keys: ProjectKeys): QueryKey[] {
  const [area] = hint.type.split(".");
  if (area === "issue" || area === "comment") return hint.key ? [keys.lists, keys.issue(hint.key)] : [keys.lists];
  if (area === "board" || area === "sprint") return [keys.lists];
  if (area === "project" || area === "version") return [keys.project, keys.lists];
  return [];
}
