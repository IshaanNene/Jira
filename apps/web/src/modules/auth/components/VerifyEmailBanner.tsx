import { MailWarning } from "lucide-react";
import { useState } from "react";

import { toast } from "@/components/ui";
import { ApiError, errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

import { useSendVerification } from "../api";

/** Shown until the person verifies their address: invitations cannot be accepted before then. */
export function VerifyEmailBanner() {
  const account = useSession((s) => s.account);
  const send = useSendVerification();
  const [sent, setSent] = useState(false);
  if (!account || account.email_verified) return null;
  return (
    <div role="status" className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-warning/30 bg-warning-soft px-4 py-2 text-sm text-ink">
      <MailWarning className="h-4 w-4 shrink-0 text-warning" aria-hidden />
      <span>
        Verify your email address: we sent a link to <strong className="font-medium">{account.email}</strong>.
      </span>
      <ResendButton
        sent={sent}
        busy={send.isPending}
        onClick={() =>
          send.mutate(undefined, {
            onSuccess: () => {
              setSent(true);
              toast.success("Link sent", `Check ${account.email}.`);
            },
            onError: (e) => toast.error("Couldn't send the link", e instanceof ApiError && e.status === 429 ? "One was sent a moment ago. Try again in a minute." : errorMessage(e)),
          })
        }
      />
    </div>
  );
}

export function ResendButton({ sent, busy, onClick }: { sent: boolean; busy: boolean; onClick: () => void }) {
  return (
    <button type="button" disabled={busy} onClick={onClick} className="font-medium text-accent hover:underline disabled:opacity-60">
      {sent ? "Send another link" : "Send a new link"}
    </button>
  );
}
