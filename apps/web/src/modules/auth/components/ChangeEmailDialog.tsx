import { zodResolver } from "@hookform/resolvers/zod";
import { MailCheck } from "lucide-react";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";

import { Button, Dialog, DialogContent, DialogFooter, Field, Input } from "@/components/ui";
import { ApiError } from "@/services/api";

import { useChangeEmail } from "../api";
import { applyServerErrors } from "../form-errors";
import { FormError } from "./FormError";

const schema = z.object({
  new_email: z.string().trim().min(1, "Enter the new address.").email("Enter a valid email address.").max(254, "Email is too long."),
  password: z.string().min(1, "Enter your current password."),
});
type Values = z.infer<typeof schema>;

/** Change the sign-in address: confirmed by a link sent to the new one. */
export function ChangeEmailDialog({ current }: { current: string }) {
  const [open, setOpen] = useState(false);
  const [sentTo, setSentTo] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const change = useChangeEmail();
  const { register, handleSubmit, setError, reset, formState: { errors, isSubmitting } } = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: { new_email: "", password: "" },
  });

  const close = (next: boolean) => {
    setOpen(next);
    if (!next) {
      reset();
      setSentTo(null);
      setFormError(null);
    }
  };

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    const new_email = values.new_email.trim().toLowerCase();
    if (new_email === current.toLowerCase()) {
      setError("new_email", { message: "This is already your address." });
      return;
    }
    try {
      await change.mutateAsync({ new_email, password: values.password });
      setSentTo(new_email);
    } catch (error) {
      if (error instanceof ApiError && error.status === 409) setError("new_email", { message: "Another account already uses this address." });
      else if (error instanceof ApiError && error.status === 429) setFormError("A link was sent a moment ago. Try again in a minute.");
      else if (error instanceof ApiError && error.status === 503) setFormError("Email isn't set up on this server, so the address can't be changed.");
      else setFormError(applyServerErrors(error, setError, ["new_email", "password"]));
    }
  });

  return (
    <Dialog open={open} onOpenChange={close}>
      <Button size="sm" onClick={() => setOpen(true)}>
        Change email
      </Button>
      {open && (
        <DialogContent
          title={sentTo ? "Check your inbox" : "Change your email"}
          description={sentTo ? undefined : `You sign in with ${current}. We'll send a link to the new address to confirm it.`}
        >
          {sentTo ? (
            <>
              <div className="flex gap-3 text-base text-ink">
                <MailCheck className="mt-0.5 h-5 w-5 shrink-0 text-success" aria-hidden />
                <p>
                  We sent a link to <span className="font-medium">{sentTo}</span>. Your address stays {current} until you open it; the link
                  expires in 24 hours.
                </p>
              </div>
              <DialogFooter>
                <Button variant="primary" onClick={() => close(false)}>Done</Button>
              </DialogFooter>
            </>
          ) : (
            <form onSubmit={onSubmit} noValidate className="space-y-4">
              <FormError message={formError} />
              <Field label="New email address" error={errors.new_email?.message}>
                <Input type="email" autoComplete="email" autoFocus {...register("new_email")} />
              </Field>
              <Field label="Current password" error={errors.password?.message}>
                <Input type="password" autoComplete="current-password" {...register("password")} />
              </Field>
              <DialogFooter>
                <Button type="button" onClick={() => close(false)}>Cancel</Button>
                <Button type="submit" variant="primary" loading={isSubmitting}>Send link</Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      )}
    </Dialog>
  );
}
