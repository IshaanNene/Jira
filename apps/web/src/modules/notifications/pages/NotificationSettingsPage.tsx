import { Mail } from "lucide-react";

import { Card, CardHeader, ErrorState, Page, Skeleton, toast } from "@/components/ui";
import type { NotificationPreferences } from "@/services/api";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";

import { usePreferences, useSavePreferences } from "../api";

type KindKey = Exclude<keyof NotificationPreferences, "email_enabled">;

const KINDS: { key: KindKey; label: string; description: string }[] = [
  { key: "email_assigned", label: "Assigned to me", description: "Someone makes you the assignee of an issue." },
  { key: "email_mentioned", label: "Mentions", description: "Someone @mentions you in a comment." },
  { key: "email_commented", label: "Comments", description: "A comment on an issue you reported, are assigned or watch." },
  { key: "email_status_changed", label: "Status changes", description: "An issue you reported, are assigned or watch changes status." },
  { key: "email_filter_digest", label: "Saved filter subscriptions", description: "The scheduled results of saved filters you subscribed to." },
];

function Toggle({ id, checked, disabled, onChange, label }: { id: string; checked: boolean; disabled?: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50",
        checked ? "bg-accent" : "bg-line-strong",
      )}
    >
      <span className={cn("inline-block h-4 w-4 rounded-full bg-white shadow transition-transform", checked ? "translate-x-[1.125rem]" : "translate-x-0.5")} />
    </button>
  );
}

/** Which notifications are also emailed, for this workspace. In-app notifications are always on. */
export default function NotificationSettingsPage() {
  const prefs = usePreferences();
  const save = useSavePreferences();
  const email = useSession((s) => s.account?.email);

  const change = (patch: Partial<NotificationPreferences>) => {
    if (!prefs.data) return;
    save.mutate({ ...prefs.data, ...patch }, { onError: () => toast.error("Couldn't save your notification settings. Try again.") });
  };

  return (
    <Page title="Notifications" description="Choose what you're emailed about in this workspace. You always see notifications in the app." width="narrow">
      {prefs.isPending ? (
        <Card className="space-y-3 p-4"><Skeleton className="h-9" /><Skeleton className="h-9" /><Skeleton className="h-9" /></Card>
      ) : prefs.isError ? (
        <Card><ErrorState error={prefs.error} onRetry={() => void prefs.refetch()} /></Card>
      ) : (
        <Card>
          <CardHeader title="Email" />
          <div className="flex items-start gap-3 border-b border-line px-4 py-4">
            <Mail className="mt-0.5 h-4 w-4 shrink-0 text-ink-muted" aria-hidden />
            <label htmlFor="email-enabled" className="min-w-0 flex-1 cursor-pointer">
              <span className="block text-sm font-medium text-ink">Send me email notifications</span>
              <span className="block text-xs text-ink-muted">
                To {email ?? "your account email"}. Emails wait a moment and are skipped if you've already read the notification.
              </span>
            </label>
            <Toggle id="email-enabled" label="Send me email notifications" checked={prefs.data.email_enabled} onChange={(v) => change({ email_enabled: v })} />
          </div>
          <ul className={cn("divide-y divide-line", !prefs.data.email_enabled && "opacity-60")}>
            {KINDS.map((k) => (
              <li key={k.key} className="flex items-start gap-3 px-4 py-3 pl-11">
                <label htmlFor={k.key} className="min-w-0 flex-1 cursor-pointer">
                  <span className="block text-sm text-ink">{k.label}</span>
                  <span className="block text-xs text-ink-muted">{k.description}</span>
                </label>
                <Toggle id={k.key} label={k.label} checked={prefs.data[k.key] ?? true} disabled={!prefs.data.email_enabled} onChange={(v) => change({ [k.key]: v })} />
              </li>
            ))}
          </ul>
        </Card>
      )}
    </Page>
  );
}
