import { Fingerprint, KeyRound, Pencil, Trash2 } from "lucide-react";
import { useState } from "react";

import { Badge, Button, Card, CardHeader, Dialog, DialogContent, EmptyState, ErrorState, Field, Input, Skeleton, toast } from "@/components/ui";
import { PasskeyCancelled, passkeysSupported, useAddPasskey, usePasskeys, useRemovePasskey, useRenamePasskey } from "@/modules/auth";
import { errorMessage, type Passkey } from "@/services/api";
import { formatDate, formatRelative } from "@/utils/format";

import { PasswordStep } from "./TwoStepVerification";

/** A name to start from: the kind of device the person is on. */
function suggestedName(): string {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return "Android phone";
  if (/Mac OS X/.test(ua)) return "Mac";
  if (/Windows/.test(ua)) return "Windows PC";
  return "Passkey";
}

/** Settings → Security → Passkeys: sign in with the device's fingerprint, face or PIN instead of a password. */
export function Passkeys() {
  const passkeys = usePasskeys();
  const [dialog, setDialog] = useState<{ kind: "add" } | { kind: "rename" | "remove"; passkey: Passkey } | null>(null);
  const close = () => setDialog(null);
  const supported = passkeysSupported();

  return (
    <Card>
      <CardHeader
        title="Passkeys"
        description="Sign in with your fingerprint, face or device PIN instead of a password. A passkey sign-in also counts as two-step verification."
        actions={supported && <Button size="sm" variant="primary" onClick={() => setDialog({ kind: "add" })}>Add a passkey</Button>}
      />
      {passkeys.isPending ? (
        <div className="p-4" aria-busy><Skeleton className="h-10" /></div>
      ) : passkeys.isError ? (
        <ErrorState error={passkeys.error} onRetry={() => void passkeys.refetch()} />
      ) : passkeys.data.length === 0 ? (
        <EmptyState
          icon={Fingerprint}
          title="No passkeys yet"
          description={supported ? "Add one for this device, or a security key, to sign in without typing your password." : "This browser can't create passkeys."}
        />
      ) : (
        <ul className="divide-y divide-line" aria-label="Passkeys">
          {passkeys.data.map((passkey) => (
            <li key={passkey.id} className="flex items-center gap-3 px-4 py-3">
              <KeyRound className="h-5 w-5 shrink-0 text-ink-subtle" aria-hidden />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate font-medium text-ink">{passkey.name}</span>
                  {passkey.synced && <Badge>Synced</Badge>}
                </div>
                <p className="text-sm text-ink-muted">
                  Added {formatDate(passkey.created_at)} · {passkey.last_used_at ? `last used ${formatRelative(passkey.last_used_at)}` : "never used"}
                </p>
              </div>
              <Button size="icon" variant="ghost" aria-label={`Rename ${passkey.name}`} onClick={() => setDialog({ kind: "rename", passkey })}>
                <Pencil className="h-4 w-4" aria-hidden />
              </Button>
              <Button size="icon" variant="ghost" aria-label={`Remove ${passkey.name}`} onClick={() => setDialog({ kind: "remove", passkey })}>
                <Trash2 className="h-4 w-4" aria-hidden />
              </Button>
            </li>
          ))}
        </ul>
      )}
      <Dialog open={dialog !== null} onOpenChange={(open) => !open && close()}>
        {dialog?.kind === "add" && <AddDialog onDone={close} />}
        {dialog?.kind === "rename" && <RenameDialog passkey={dialog.passkey} onDone={close} />}
        {dialog?.kind === "remove" && <RemoveDialog passkey={dialog.passkey} onDone={close} />}
      </Dialog>
    </Card>
  );
}

function AddDialog({ onDone }: { onDone: () => void }) {
  const add = useAddPasskey();
  const [name, setName] = useState(suggestedName);
  // Cancelling the browser's prompt just leaves the dialog open to try again.
  const error = add.error instanceof PasskeyCancelled ? null : add.error;
  return (
    <DialogContent title="Add a passkey" description="Enter your password, then follow your browser's prompt.">
      <PasswordStep
        busy={add.isPending}
        error={error}
        submitLabel="Continue"
        onSubmit={(password) =>
          add.mutate(
            { password, name: name.trim() || suggestedName() },
            {
              onSuccess: (passkey) => {
                toast.success("Passkey added", `You can now sign in with “${passkey.name}”.`);
                onDone();
              },
            },
          )
        }
      >
        <Field label="Name" hint="So you can tell your passkeys apart.">
          <Input value={name} maxLength={60} onChange={(e) => setName(e.target.value)} />
        </Field>
      </PasswordStep>
    </DialogContent>
  );
}

function RenameDialog({ passkey, onDone }: { passkey: Passkey; onDone: () => void }) {
  const rename = useRenamePasskey();
  const [name, setName] = useState(passkey.name);
  return (
    <DialogContent title="Rename passkey">
      <form
        noValidate
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          rename.mutate({ id: passkey.id, name }, { onSuccess: onDone, onError: (err) => toast.error("Couldn't rename the passkey", errorMessage(err)) });
        }}
      >
        <Field label="Name">
          <Input value={name} maxLength={60} autoFocus onChange={(e) => setName(e.target.value)} />
        </Field>
        <div className="flex justify-end">
          <Button type="submit" variant="primary" loading={rename.isPending} disabled={!name.trim()}>Save</Button>
        </div>
      </form>
    </DialogContent>
  );
}

function RemoveDialog({ passkey, onDone }: { passkey: Passkey; onDone: () => void }) {
  const remove = useRemovePasskey();
  return (
    <DialogContent title={`Remove “${passkey.name}”?`} description="It stops working at once. Also delete it from the device or password manager that holds it.">
      <PasswordStep
        busy={remove.isPending}
        error={remove.error}
        submitLabel="Remove passkey"
        onSubmit={(password) =>
          remove.mutate({ id: passkey.id, password }, {
            onSuccess: () => {
              toast.success("Passkey removed");
              onDone();
            },
          })
        }
      />
    </DialogContent>
  );
}
