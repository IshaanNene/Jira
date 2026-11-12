import { MoreHorizontal, Search, Users } from "lucide-react";
import { useDeferredValue, useState } from "react";
import { useNavigate } from "react-router-dom";

import {
  Avatar,
  Badge,
  Button,
  Card,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  EmptyState,
  ErrorState,
  Input,
  Select,
  Skeleton,
  toast,
} from "@/components/ui";
import { leaveOrganizationScope } from "@/modules/auth";
import { errorMessage, type Member, type OrgRole } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate } from "@/utils/format";

import { useMembers, useRemoveMember, useUpdateMember } from "../api";
import { assignableRoles, can, ROLE_LABEL } from "../permissions";
import { RoleBadge } from "./RoleBadge";

export function MembersTable() {
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search.trim());
  const members = useMembers(deferredSearch);
  const rows = members.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <Card>
      <div className="flex items-center gap-3 border-b border-line px-4 py-3">
        <h2 className="text-base font-semibold text-ink">Members</h2>
        {members.isSuccess && !deferredSearch && !members.hasNextPage && <Badge>{rows.length}</Badge>}
        <div className="relative ml-auto w-64 max-w-full">
          <Search className="pointer-events-none absolute left-2.5 top-2 h-4 w-4 text-ink-subtle" aria-hidden />
          <Input
            type="search"
            aria-label="Search members"
            placeholder="Search by name or email"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="h-8 pl-8 text-sm"
          />
        </div>
      </div>

      {members.isPending ? (
        <div className="divide-y divide-line" aria-busy aria-label="Loading members">
          {Array.from({ length: 4 }, (_, i) => (
            <div key={i} className="flex items-center gap-3 px-4 py-3">
              <Skeleton className="h-8 w-8 rounded-full" />
              <div className="flex-1 space-y-1.5">
                <Skeleton className="h-3.5 w-40" />
                <Skeleton className="h-3 w-56" />
              </div>
              <Skeleton className="h-6 w-20" />
            </div>
          ))}
        </div>
      ) : members.isError ? (
        <ErrorState error={members.error} onRetry={() => void members.refetch()} />
      ) : rows.length === 0 ? (
        <EmptyState icon={Users} title="No one matches" description={`No member's name or email contains “${deferredSearch}”.`} />
      ) : (
        <>
          <table className="w-full table-fixed text-left">
            <thead>
              <tr className="border-b border-line text-2xs font-semibold uppercase tracking-wide text-ink-subtle">
                <th scope="col" className="px-4 py-2 font-semibold">Person</th>
                <th scope="col" className="w-40 px-2 py-2 font-semibold">Role</th>
                <th scope="col" className="hidden w-32 px-2 py-2 font-semibold sm:table-cell">Joined</th>
                <th scope="col" className="w-12 px-2 py-2"><span className="sr-only">Actions</span></th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {rows.map((member) => (
                <MemberRow key={member.user_id} member={member} />
              ))}
            </tbody>
          </table>
          {members.hasNextPage && (
            <div className="border-t border-line p-2 text-center">
              <Button variant="ghost" size="sm" loading={members.isFetchingNextPage} onClick={() => void members.fetchNextPage()}>
                Load more
              </Button>
            </div>
          )}
        </>
      )}
    </Card>
  );
}

function MemberRow({ member }: { member: Member }) {
  const navigate = useNavigate();
  const me = useSession((s) => s.account?.id);
  const myRole = useSession((s) => s.organization?.role);
  const update = useUpdateMember();
  const remove = useRemoveMember();
  const isMe = member.user_id === me;
  const role = member.org_role as OrgRole;
  const roles = assignableRoles(myRole, role);
  const suspended = member.status === "suspended";
  // Admins manage non-owners; owners manage everyone. The server re-checks all of this.
  const canManage = can(myRole, "manageMembers") && (role !== "owner" || can(myRole, "manageOwnership"));
  const busy = update.isPending || remove.isPending;

  const report = (title: string) => (error: unknown) => toast.error(title, errorMessage(error));

  const changeRole = (next: string) =>
    update.mutate(
      { userId: member.user_id, org_role: next as OrgRole },
      { onSuccess: () => toast.success(`${member.display_name} is now ${ROLE_LABEL[next as OrgRole]}`), onError: report("Couldn't change role") },
    );

  const setStatus = (status: "active" | "suspended") =>
    update.mutate(
      { userId: member.user_id, status },
      {
        onSuccess: () => toast.success(status === "suspended" ? `${member.display_name} was suspended` : `${member.display_name} was reactivated`),
        onError: report("Couldn't update member"),
      },
    );

  const removeMember = () =>
    remove.mutate(member.user_id, {
      onSuccess: async () => {
        if (isMe) {
          // Left the workspace: this session no longer has an organization here.
          await leaveOrganizationScope();
          navigate("/onboarding", { replace: true });
        } else {
          toast.success(`${member.display_name} was removed`);
        }
      },
      onError: report(isMe ? "Couldn't leave workspace" : "Couldn't remove member"),
    });

  return (
    <tr className={suspended ? "opacity-60" : undefined}>
      <td className="px-4 py-2.5">
        <div className="flex min-w-0 items-center gap-3">
          <Avatar name={member.display_name} seed={member.user_id} />
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="truncate text-base font-medium text-ink">{member.display_name}</span>
              {isMe && <Badge>You</Badge>}
              {suspended && <Badge tone="warning">Suspended</Badge>}
            </div>
            <div className="truncate text-sm text-ink-muted">{member.email}</div>
          </div>
        </div>
      </td>
      <td className="px-2 py-2.5">
        {roles.length > 0 && !suspended ? (
          <Select
            size="sm"
            aria-label={`Role for ${member.display_name}`}
            value={role}
            disabled={busy}
            onValueChange={changeRole}
            options={roles.map((r) => ({ value: r, label: ROLE_LABEL[r] }))}
          />
        ) : (
          <RoleBadge role={member.org_role} />
        )}
      </td>
      <td className="hidden px-2 py-2.5 text-sm text-ink-muted sm:table-cell">{formatDate(member.joined_at)}</td>
      <td className="px-2 py-2.5 text-right">
        {(canManage || isMe) && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" aria-label={`Actions for ${member.display_name}`} disabled={busy}>
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {canManage && !isMe && (
                <DropdownMenuItem onSelect={() => setStatus(suspended ? "active" : "suspended")}>
                  {suspended ? "Reactivate" : "Suspend"}
                </DropdownMenuItem>
              )}
              {isMe ? (
                <DropdownMenuItem tone="danger" onSelect={removeMember}>Leave workspace</DropdownMenuItem>
              ) : (
                canManage && <DropdownMenuItem tone="danger" onSelect={removeMember}>Remove from workspace</DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </td>
    </tr>
  );
}
