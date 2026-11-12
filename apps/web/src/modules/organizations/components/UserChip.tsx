import { UserRound } from "lucide-react";

import { Avatar } from "@/components/ui";
import { cn } from "@/utils/cn";

import { useUserDirectory } from "../api";

/** Avatar (and optionally name) for a user id from another service. */
export function UserChip({ userId, showName = true, size = "sm" }: { userId: string | null | undefined; showName?: boolean; size?: "sm" | "md" }) {
  const { byId } = useUserDirectory();
  if (!userId) {
    return (
      <span className="inline-flex items-center gap-2 text-sm text-ink-muted">
        <span className={cn("inline-flex items-center justify-center rounded-full bg-sunken text-ink-subtle", size === "sm" ? "h-6 w-6" : "h-8 w-8")}>
          <UserRound className="h-3.5 w-3.5" aria-hidden />
        </span>
        {showName ? "Unassigned" : <span className="sr-only">Unassigned</span>}
      </span>
    );
  }
  const name = byId.get(userId)?.display_name ?? "Former member";
  return (
    <span className="inline-flex min-w-0 items-center gap-2 text-sm text-ink" title={name}>
      <Avatar name={name} seed={userId} size={size} />
      {showName ? <span className="truncate">{name}</span> : <span className="sr-only">{name}</span>}
    </span>
  );
}
