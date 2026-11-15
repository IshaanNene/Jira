import { ChevronsUpDown, Plus } from "lucide-react";
import { useNavigate } from "react-router-dom";

import {
  Avatar,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Skeleton,
  toast,
} from "@/components/ui";
import { leaveOrganizationScope, switchOrganization, twoStepRequiredPath } from "@/modules/auth";
import { useCurrentOrganization, useMyOrganizations } from "@/modules/organizations";
import { errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

export function WorkspaceSwitcher({ collapsed }: { collapsed: boolean }) {
  const navigate = useNavigate();
  const activeId = useSession((s) => s.organization?.id);
  const current = useCurrentOrganization();
  const mine = useMyOrganizations();

  const switchTo = async (id: string) => {
    if (id === activeId) return;
    try {
      await switchOrganization(id);
      navigate("/");
    } catch (error) {
      const required = twoStepRequiredPath(error, id);
      if (required) return navigate(required);
      toast.error("Couldn't switch workspace", errorMessage(error));
    }
  };

  const createNew = async () => {
    await leaveOrganizationScope();
    navigate("/onboarding");
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="flex h-10 w-full items-center gap-2 rounded px-1.5 text-left transition-colors hover:bg-sunken"
        aria-label="Switch workspace"
      >
        {current.data ? <Avatar name={current.data.name} seed={current.data.id} square size="md" /> : <Skeleton className="h-8 w-8" />}
        {!collapsed && (
          <>
            <span className="min-w-0 flex-1">
              {current.data ? <span className="block truncate text-base font-semibold text-ink">{current.data.name}</span> : <Skeleton className="h-4 w-24" />}
            </span>
            <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 text-ink-subtle" aria-hidden />
          </>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        <DropdownMenuLabel>Workspaces</DropdownMenuLabel>
        {(mine.data ?? []).map((org) => (
          <DropdownMenuItem key={org.id} selected={org.id === activeId} onSelect={() => void switchTo(org.id)}>
            <Avatar name={org.name} seed={org.id} square size="sm" />
            <span className="truncate">{org.name}</span>
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void createNew()}>
          <Plus className="h-4 w-4 text-ink-muted" aria-hidden /> Create workspace
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
