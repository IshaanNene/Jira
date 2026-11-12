import { zodResolver } from "@hookform/resolvers/zod";
import { Check, Copy, UserPlus } from "lucide-react";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";

import { Button, Dialog, DialogContent, DialogFooter, DialogTrigger, Field, Input, Select } from "@/components/ui";
import { applyServerErrors, FormError } from "@/modules/auth";
import type { Invitation, OrgRole } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate } from "@/utils/format";

import { useCreateInvitation } from "../api";
import { invitableRoles, ROLE_DESCRIPTION, ROLE_LABEL } from "../permissions";

const schema = z.object({
  email: z.string().trim().min(1, "Enter an email address.").email("Enter a valid email address."),
  org_role: z.enum(["admin", "member", "guest"]),
});
type Values = z.infer<typeof schema>;

export function InviteDialog() {
  const role = useSession((s) => s.organization?.role);
  const create = useCreateInvitation();
  const [open, setOpen] = useState(false);
  const [created, setCreated] = useState<Invitation | null>(null);
  const [copied, setCopied] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const {
    register,
    handleSubmit,
    control,
    reset,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { email: "", org_role: "member" } });

  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) {
      // Reset on close so the one-time link is not left in memory or shown again.
      setCreated(null);
      setCopied(false);
      setFormError(null);
      reset();
    }
  };

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      setCreated(await create.mutateAsync({ email: values.email, org_role: values.org_role as OrgRole }));
    } catch (error) {
      setFormError(applyServerErrors(error, setError, ["email", "org_role"]));
    }
  });

  const copy = async () => {
    if (!created?.accept_url) return;
    await navigator.clipboard.writeText(created.accept_url);
    setCopied(true);
  };

  const roles = invitableRoles(role).map((r) => ({ value: r, label: ROLE_LABEL[r], description: ROLE_DESCRIPTION[r] }));

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogTrigger asChild>
        <Button variant="primary">
          <UserPlus className="h-3.5 w-3.5" aria-hidden /> Invite people
        </Button>
      </DialogTrigger>
      {created ? (
        <DialogContent title="Invitation created" description={`${created.email} can join as ${ROLE_LABEL[created.org_role as OrgRole] ?? created.org_role} until ${formatDate(created.expires_at)}.`}>
          <Field
            label="Invitation link"
            hint="Share this link with them directly. It is shown only once and works only for the invited email address."
          >
            <Input readOnly value={created.accept_url ?? ""} onFocus={(e) => e.currentTarget.select()} className="font-mono text-sm" />
          </Field>
          <DialogFooter>
            <Button onClick={() => void copy()}>
              {copied ? <Check className="h-3.5 w-3.5 text-success" aria-hidden /> : <Copy className="h-3.5 w-3.5" aria-hidden />}
              {copied ? "Copied" : "Copy link"}
            </Button>
            <Button variant="primary" onClick={() => onOpenChange(false)}>
              Done
            </Button>
          </DialogFooter>
        </DialogContent>
      ) : (
        <DialogContent title="Invite people" description="They will join this workspace with the role you choose.">
          <form onSubmit={onSubmit} noValidate className="space-y-4">
            <FormError message={formError} />
            <Field label="Email address" error={errors.email?.message}>
              <Input type="email" autoFocus autoComplete="off" placeholder="name@company.com" {...register("email")} />
            </Field>
            <Controller
              control={control}
              name="org_role"
              render={({ field }) => (
                <Field label="Role" error={errors.org_role?.message}>
                  <Select value={field.value} onValueChange={field.onChange} options={roles} />
                </Field>
              )}
            />
            <DialogFooter>
              <Button onClick={() => onOpenChange(false)}>Cancel</Button>
              <Button type="submit" variant="primary" loading={isSubmitting}>
                Create invitation
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      )}
    </Dialog>
  );
}
