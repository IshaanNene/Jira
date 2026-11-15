import { ChevronRight, LogOut } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";

import { Avatar, Button, ErrorState, Skeleton, toast } from "@/components/ui";
import { AuthLayout, signOut, switchOrganization, twoStepRequiredPath } from "@/modules/auth";
import { errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

import { useMyOrganizations } from "../api";
import { CreateOrganizationForm } from "../components/CreateOrganizationForm";
import { RoleBadge } from "../components/RoleBadge";

/** First stop after signing in without an active workspace: pick one or create one. */
export default function OnboardingPage() {
  const navigate = useNavigate();
  const account = useSession((s) => s.account);
  const organizations = useMyOrganizations();
  const [creating, setCreating] = useState(false);
  const [opening, setOpening] = useState<string | null>(null);

  const open = async (organizationId: string) => {
    setOpening(organizationId);
    try {
      await switchOrganization(organizationId);
      navigate("/", { replace: true });
    } catch (error) {
      const required = twoStepRequiredPath(error, organizationId);
      if (required) return navigate(required);
      toast.error("Couldn't open workspace", errorMessage(error));
      setOpening(null);
    }
  };

  const footer = (
    <span className="flex items-center justify-between">
      <span className="truncate">Signed in as {account?.email}</span>
      <button type="button" onClick={() => void signOut()} className="inline-flex items-center gap-1 font-medium text-ink-muted hover:text-ink">
        <LogOut className="h-3.5 w-3.5" aria-hidden /> Sign out
      </button>
    </span>
  );

  if (organizations.isPending) {
    return (
      <AuthLayout title="Loading your workspaces" footer={footer}>
        <div className="space-y-2" aria-busy>
          <Skeleton className="h-14" />
          <Skeleton className="h-14" />
        </div>
      </AuthLayout>
    );
  }
  if (organizations.isError) {
    return (
      <AuthLayout title="Your workspaces" footer={footer}>
        <ErrorState error={organizations.error} onRetry={() => void organizations.refetch()} />
      </AuthLayout>
    );
  }

  const items = organizations.data;
  if (items.length === 0 || creating) {
    return (
      <AuthLayout
        title="Create your workspace"
        subtitle="A workspace holds your organization's people, projects and settings. You can invite teammates next."
        footer={footer}
      >
        <CreateOrganizationForm onCreated={open} />
        {items.length > 0 && (
          <Button variant="ghost" className="mt-2 w-full" onClick={() => setCreating(false)}>
            Back to my workspaces
          </Button>
        )}
      </AuthLayout>
    );
  }

  return (
    <AuthLayout title="Choose a workspace" subtitle="You belong to more than one. Pick where to work now." footer={footer}>
      <ul className="divide-y divide-line overflow-hidden rounded-lg border border-line bg-surface shadow-xs">
        {items.map((org) => (
          <li key={org.id}>
            <button
              type="button"
              disabled={opening !== null}
              onClick={() => void open(org.id)}
              className="flex w-full items-center gap-3 px-3 py-2.5 text-left transition-colors hover:bg-sunken disabled:opacity-60"
            >
              <Avatar name={org.name} seed={org.id} square />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-base font-medium text-ink">{org.name}</span>
                <span className="block truncate text-xs text-ink-subtle">{org.slug}</span>
              </span>
              <RoleBadge role={org.role} />
              <ChevronRight className="h-4 w-4 text-ink-subtle" aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      <Button className="mt-3 w-full" onClick={() => setCreating(true)}>
        Create another workspace
      </Button>
    </AuthLayout>
  );
}
