import { Bell, CheckCheck } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";

import { Avatar, Button, DropdownMenu, DropdownMenuContent, DropdownMenuTrigger, EmptyState, ErrorState, Skeleton, Tooltip } from "@/components/ui";
import type { AppNotification } from "@/services/api";
import { cn } from "@/utils/cn";
import { formatRelative } from "@/utils/format";

import { useMarkRead, useNotificationLive, useNotifications, useUnreadCount } from "./api";
import { badgeLabel, describe, issuePath } from "./model";

function Row({ n, onOpen }: { n: AppNotification; onOpen: (n: AppNotification) => void }) {
  return (
    <li>
      <button
        type="button"
        onClick={() => onOpen(n)}
        className="flex w-full gap-3 px-4 py-3 text-left hover:bg-sunken focus-visible:bg-sunken focus-visible:outline-none"
      >
        <Avatar name={n.actor_name || "?"} size="sm" />
        <span className="min-w-0 flex-1">
          <span className="block text-sm text-ink">{describe(n)}</span>
          <span className="mt-0.5 block truncate text-sm text-ink-muted">
            <span className="font-medium">{n.issue_key}</span> {n.issue_summary}
          </span>
          <span className="mt-0.5 block text-xs text-ink-subtle">{formatRelative(n.created_at)}</span>
        </span>
        <span className={cn("mt-1.5 h-2 w-2 shrink-0 rounded-full", n.read ? "bg-transparent" : "bg-accent")} aria-label={n.read ? undefined : "Unread"} />
      </button>
    </li>
  );
}

export function NotificationBell() {
  const [open, setOpen] = useState(false);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const navigate = useNavigate();
  useNotificationLive();
  const count = useUnreadCount();
  const list = useNotifications(unreadOnly, open);
  const mark = useMarkRead();
  const badge = badgeLabel(count.data ?? 0);

  const openOne = (n: AppNotification) => {
    if (!n.read) mark.one.mutate(n.id);
    setOpen(false);
    navigate(issuePath(n.issue_key));
  };

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <Tooltip label="Notifications" side="bottom">
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon" className="relative" aria-label={badge ? `Notifications, ${count.data} unread` : "Notifications"}>
            <Bell className="h-4 w-4" />
            {badge && (
              <span className="absolute -right-0.5 -top-0.5 min-w-[1.1rem] rounded-full bg-danger px-1 text-center text-2xs font-semibold leading-[1.1rem] text-white">
                {badge}
              </span>
            )}
          </Button>
        </DropdownMenuTrigger>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-[26rem] p-0">
        <div className="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 className="text-base font-semibold text-ink">Notifications</h2>
          <label className="flex items-center gap-2 text-xs text-ink-muted">
            <input type="checkbox" checked={unreadOnly} onChange={(e) => setUnreadOnly(e.target.checked)} />
            Only show unread
          </label>
        </div>
        <div className="max-h-[28rem] overflow-y-auto">
          {list.isPending ? (
            <div className="space-y-3 p-4">
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
            </div>
          ) : list.isError ? (
            <ErrorState className="py-8" error={list.error} onRetry={() => void list.refetch()} />
          ) : list.data.items.length === 0 ? (
            <EmptyState
              className="py-10"
              icon={Bell}
              title={unreadOnly ? "No unread notifications" : "No notifications yet"}
              description="You'll hear here when someone assigns you an issue, mentions you, or changes an issue you follow."
            />
          ) : (
            <ul className="divide-y divide-line">
              {list.data.items.map((n) => (
                <Row key={n.id} n={n} onOpen={openOne} />
              ))}
            </ul>
          )}
        </div>
        {(count.data ?? 0) > 0 && (
          <div className="border-t border-line px-2 py-2">
            <Button variant="ghost" size="sm" onClick={() => mark.all.mutate()} disabled={mark.all.isPending}>
              <CheckCheck className="h-4 w-4" aria-hidden /> Mark all as read
            </Button>
          </div>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
