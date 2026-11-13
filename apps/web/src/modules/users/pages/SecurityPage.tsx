import { MonitorSmartphone } from "lucide-react";

import { Badge, Button, Card, CardHeader, EmptyState, ErrorState, Page, Skeleton, toast } from "@/components/ui";
import { useRevokeSession, useSessions } from "@/modules/auth";
import { errorMessage } from "@/services/api";
import { describeUserAgent, formatDate, formatRelative } from "@/utils/format";

import { TwoStepVerification } from "../components/TwoStepVerification";

export default function SecurityPage() {
  return (
    <Page title="Security" description="How you sign in, and where you are signed in." width="narrow">
      <div className="space-y-5">
        <TwoStepVerification />
        <Sessions />
      </div>
    </Page>
  );
}

function Sessions() {
  const sessions = useSessions();
  const revoke = useRevokeSession();
  return (
      <Card>
        <CardHeader title="Active sessions" description="Sign out a session you don't recognise. It loses access immediately." />
        {sessions.isPending ? (
          <div className="space-y-2 p-4" aria-busy><Skeleton className="h-12" /><Skeleton className="h-12" /></div>
        ) : sessions.isError ? (
          <ErrorState error={sessions.error} onRetry={() => void sessions.refetch()} />
        ) : sessions.data.items.length === 0 ? (
          <EmptyState icon={MonitorSmartphone} title="No active sessions" description="Sessions appear here after you sign in." />
        ) : (
          <ul className="divide-y divide-line">
            {sessions.data.items.map((session) => (
              <li key={session.id} className="flex items-center gap-3 px-4 py-3">
                <MonitorSmartphone className="h-5 w-5 shrink-0 text-ink-subtle" aria-hidden />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-base font-medium text-ink">{describeUserAgent(session.user_agent)}</span>
                    {session.current && <Badge tone="success">This device</Badge>}
                  </div>
                  <div className="truncate text-sm text-ink-muted">
                    {session.ip ?? "Unknown address"} · Active {formatRelative(session.last_seen_at)} · Signed in {formatDate(session.created_at)}
                  </div>
                </div>
                {!session.current && (
                  <Button
                    size="sm"
                    disabled={revoke.isPending}
                    onClick={() =>
                      revoke.mutate(session.id, {
                        onSuccess: () => toast.success("Session signed out"),
                        onError: (error) => toast.error("Couldn't sign out session", errorMessage(error)),
                      })
                    }
                  >
                    Sign out
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
      </Card>
  );
}
