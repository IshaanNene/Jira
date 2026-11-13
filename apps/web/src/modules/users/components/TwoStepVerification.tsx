import { Check, Copy, Download, KeyRound, ShieldCheck, ShieldOff } from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";

import { Badge, Button, Card, CardHeader, Dialog, DialogContent, DialogFooter, ErrorState, Field, Input, Skeleton, toast } from "@/components/ui";
import { useConfirmTotp, useDisableTotp, useMfaStatus, useRegenerateRecoveryCodes, useStartTotpSetup } from "@/modules/auth";
import { ApiError, errorMessage, type TotpSetup } from "@/services/api";
import { formatDate } from "@/utils/format";

/** The message for one field from a 422, or a general one. */
function fieldMessage(error: unknown, field: string): string | undefined {
  if (error instanceof ApiError && error.fieldErrors.length > 0) return error.fieldErrors.find((e) => e.field === field)?.message;
  return undefined;
}

function generalMessage(error: unknown): string | null {
  if (!error) return null;
  if (error instanceof ApiError && error.fieldErrors.length > 0) return null;
  return errorMessage(error);
}

/** Two-step verification status, with the dialogs that change it. */
export function TwoStepVerification() {
  const status = useMfaStatus();
  const [dialog, setDialog] = useState<"setup" | "codes" | "disable" | null>(null);
  const close = () => setDialog(null);

  return (
    <Card>
      <CardHeader
        title="Two-step verification"
        description="After your password, ask for a code from an authenticator app on your phone."
        actions={status.data?.totp_enabled ? <Badge tone="success">On</Badge> : status.data ? <Badge>Off</Badge> : null}
      />
      <div className="px-4 py-4">
        {status.isPending ? (
          <Skeleton className="h-10" />
        ) : status.isError ? (
          <ErrorState error={status.error} onRetry={() => void status.refetch()} />
        ) : status.data.totp_enabled ? (
          <div className="flex flex-wrap items-center gap-3">
            <ShieldCheck className="h-5 w-5 shrink-0 text-success" aria-hidden />
            <div className="min-w-0 flex-1 text-sm">
              <p className="text-ink">Authenticator app{status.data.enabled_at ? `, since ${formatDate(status.data.enabled_at)}` : ""}</p>
              <p className={status.data.recovery_codes_remaining <= 3 ? "text-warning" : "text-ink-muted"}>
                {status.data.recovery_codes_remaining === 0
                  ? "No recovery codes left. Make new ones so you can still sign in without your phone."
                  : `${status.data.recovery_codes_remaining} recovery ${status.data.recovery_codes_remaining === 1 ? "code" : "codes"} left`}
              </p>
            </div>
            <Button size="sm" onClick={() => setDialog("codes")}>
              <KeyRound className="h-4 w-4" aria-hidden />
              New recovery codes
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setDialog("disable")}>
              Turn off
            </Button>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-3">
            <ShieldOff className="h-5 w-5 shrink-0 text-ink-subtle" aria-hidden />
            <p className="min-w-0 flex-1 text-sm text-ink-muted">
              Anyone who learns your password can sign in as you. A code from your phone stops them.
            </p>
            <Button size="sm" variant="primary" onClick={() => setDialog("setup")}>
              Set up
            </Button>
          </div>
        )}
      </div>

      <Dialog open={dialog === "setup"} onOpenChange={(open) => !open && close()}>
        {dialog === "setup" && <SetupDialog onDone={close} />}
      </Dialog>
      <Dialog open={dialog === "codes"} onOpenChange={(open) => !open && close()}>
        {dialog === "codes" && <RegenerateDialog onDone={close} />}
      </Dialog>
      <Dialog open={dialog === "disable"} onOpenChange={(open) => !open && close()}>
        {dialog === "disable" && <DisableDialog onDone={close} />}
      </Dialog>
    </Card>
  );
}

function PasswordStep({ busy, error, submitLabel, onSubmit, children }: {
  busy: boolean;
  error: unknown;
  submitLabel: string;
  onSubmit: (password: string) => void;
  children?: ReactNode;
}) {
  const [password, setPassword] = useState("");
  return (
    <form
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        if (password) onSubmit(password);
      }}
      className="space-y-4"
    >
      {generalMessage(error) && <p role="alert" className="text-sm text-danger">{generalMessage(error)}</p>}
      <Field label="Password" error={fieldMessage(error, "password")}>
        <Input type="password" autoComplete="current-password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} />
      </Field>
      {children}
      <DialogFooter>
        <Button type="submit" variant="primary" loading={busy} disabled={!password}>
          {submitLabel}
        </Button>
      </DialogFooter>
    </form>
  );
}

function SetupDialog({ onDone }: { onDone: () => void }) {
  const start = useStartTotpSetup();
  const confirm = useConfirmTotp();
  const [code, setCode] = useState("");

  if (confirm.data) {
    return (
      <DialogContent title="Save your recovery codes" description="Two-step verification is on. You've been signed out everywhere else.">
        <RecoveryCodeList codes={confirm.data.codes} onDone={onDone} />
      </DialogContent>
    );
  }
  if (!start.data) {
    return (
      <DialogContent title="Set up two-step verification" description="Enter your password to continue.">
        <PasswordStep busy={start.isPending} error={start.error} submitLabel="Continue" onSubmit={(password) => start.mutate(password)} />
      </DialogContent>
    );
  }
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (code.trim()) confirm.mutate(code.trim());
  };
  return (
    <DialogContent title="Scan with your authenticator app" description="Any app that supports time-based codes works.">
      <form noValidate onSubmit={submit} className="space-y-4">
        <QrCode setup={start.data} />
        {generalMessage(confirm.error) && (
          <p role="alert" className="text-sm text-danger">
            {generalMessage(confirm.error)}
            {confirm.error instanceof ApiError && confirm.error.status === 409 && (
              <>
                {" "}
                <button type="button" className="font-medium underline" onClick={() => { confirm.reset(); start.reset(); setCode(""); }}>
                  Start again
                </button>
              </>
            )}
          </p>
        )}
        <Field label="Code from the app" error={fieldMessage(confirm.error, "code")}>
          <Input inputMode="numeric" autoComplete="one-time-code" maxLength={7} autoFocus placeholder="123456" value={code} onChange={(e) => setCode(e.target.value)} />
        </Field>
        <DialogFooter>
          <Button type="submit" variant="primary" loading={confirm.isPending} disabled={!code.trim()}>
            Turn on
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

function QrCode({ setup }: { setup: TotpSetup }) {
  // Rendered as an image, so nothing in the SVG can run.
  const src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(setup.qr_svg)}`;
  const grouped = setup.secret.match(/.{1,4}/g)?.join(" ") ?? setup.secret;
  return (
    <div className="flex flex-col items-center gap-3 sm:flex-row sm:items-start">
      <img src={src} alt="QR code to add this account to an authenticator app" className="h-44 w-44 shrink-0 rounded border border-line bg-white p-1" />
      <div className="min-w-0 text-sm text-ink-muted">
        <p>Can't scan it? Enter this key in the app instead:</p>
        <p className="mt-1 break-all font-mono text-sm text-ink" aria-label="Setup key">{grouped}</p>
        <p className="mt-2">Then type the 6-digit code the app shows.</p>
      </div>
    </div>
  );
}

function RecoveryCodeList({ codes, onDone }: { codes: string[]; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const text = `Meridian recovery codes\nEach code works once in place of an authenticator code.\n\n${codes.join("\n")}\n`;
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      toast.error("Couldn't copy", "Select the codes and copy them instead.");
    }
  };
  const save = () => {
    const href = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const link = document.createElement("a");
    link.href = href;
    link.download = "meridian-recovery-codes.txt";
    link.click();
    URL.revokeObjectURL(href);
  };
  return (
    <div className="space-y-4">
      <p className="text-sm text-ink-muted">
        If you lose your phone, each of these codes lets you sign in once. Keep them somewhere safe. You won't see them again.
      </p>
      <ul aria-label="Recovery codes" className="grid grid-cols-2 gap-x-6 gap-y-1 rounded border border-line bg-sunken px-4 py-3 font-mono text-sm text-ink">
        {codes.map((code) => (
          <li key={code}>{code}</li>
        ))}
      </ul>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" onClick={() => void copy()}>
          {copied ? <Check className="h-4 w-4" aria-hidden /> : <Copy className="h-4 w-4" aria-hidden />}
          {copied ? "Copied" : "Copy"}
        </Button>
        <Button size="sm" onClick={save}>
          <Download className="h-4 w-4" aria-hidden />
          Download
        </Button>
      </div>
      <DialogFooter>
        <Button variant="primary" onClick={onDone}>
          I've saved them
        </Button>
      </DialogFooter>
    </div>
  );
}

function CodeField({ error, value, onChange }: { error: unknown; value: string; onChange: (value: string) => void }) {
  return (
    <Field label="Authentication or recovery code" error={fieldMessage(error, "code")}>
      <Input autoComplete="one-time-code" spellCheck={false} placeholder="123456" value={value} onChange={(e) => onChange(e.target.value)} />
    </Field>
  );
}

function RegenerateDialog({ onDone }: { onDone: () => void }) {
  const regenerate = useRegenerateRecoveryCodes();
  const [code, setCode] = useState("");
  if (regenerate.data) {
    return (
      <DialogContent title="Your new recovery codes" description="The old codes no longer work.">
        <RecoveryCodeList codes={regenerate.data.codes} onDone={onDone} />
      </DialogContent>
    );
  }
  return (
    <DialogContent title="Make new recovery codes" description="Your current codes stop working.">
      <PasswordStep busy={regenerate.isPending} error={regenerate.error} submitLabel="Make new codes"
        onSubmit={(password) => code.trim() && regenerate.mutate({ password, code: code.trim() })}>
        <CodeField error={regenerate.error} value={code} onChange={setCode} />
      </PasswordStep>
    </DialogContent>
  );
}

function DisableDialog({ onDone }: { onDone: () => void }) {
  const disable = useDisableTotp();
  const [code, setCode] = useState("");
  return (
    <DialogContent title="Turn off two-step verification?" description="Your password alone will be enough to sign in. You'll be signed out everywhere else.">
      <PasswordStep busy={disable.isPending} error={disable.error} submitLabel="Turn off"
        onSubmit={(password) =>
          code.trim() &&
          disable.mutate(
            { password, code: code.trim() },
            {
              onSuccess: () => {
                toast.success("Two-step verification is off");
                onDone();
              },
            },
          )
        }>
        <CodeField error={disable.error} value={code} onChange={setCode} />
      </PasswordStep>
    </DialogContent>
  );
}
