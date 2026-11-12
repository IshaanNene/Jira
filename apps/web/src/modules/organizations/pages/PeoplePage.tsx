import { Lock } from "lucide-react";

import { EmptyState, Page } from "@/components/ui";
import { useSession } from "@/state/session";

import { InvitationsCard } from "../components/InvitationsCard";
import { InviteDialog } from "../components/InviteDialog";
import { MembersTable } from "../components/MembersTable";
import { can } from "../permissions";

export default function PeoplePage() {
  const role = useSession((s) => s.organization?.role);
  if (!can(role, "viewMembers")) {
    return (
      <Page title="People">
        <EmptyState icon={Lock} title="The member directory isn't shared with guests" description="Ask a workspace admin if you need to find someone." />
      </Page>
    );
  }
  const canInvite = can(role, "inviteMembers");
  return (
    <Page title="People" description="Everyone with access to this workspace." actions={canInvite ? <InviteDialog /> : undefined}>
      <div className="space-y-5">
        <MembersTable />
        {canInvite && <InvitationsCard />}
      </div>
    </Page>
  );
}
