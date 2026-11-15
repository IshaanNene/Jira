import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { Link, useSearchParams } from "react-router-dom";
import { z } from "zod";

import { Button, Field, Input } from "@/components/ui";
import { ApiError, errorMessage } from "@/services/api";
import { useSession } from "@/state/session";

import { useResetPassword } from "../api";
import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";
import { signOut } from "../session-service";

const schema = z
  .object({
    password: z.string().min(10, "Use at least 10 characters.").max(128, "Use at most 128 characters."),
    confirm: z.string(),
  })
  .refine((v) => v.password === v.confirm, { path: ["confirm"], message: "The passwords don't match." });
type Values = z.infer<typeof schema>;

/** Landing page for the link in a password reset email. */
export default function ResetPasswordPage() {
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";
  const reset = useResetPassword();
  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { password: "", confirm: "" } });

  const onSubmit = handleSubmit((values) =>
    reset.mutate(
      { token, password: values.password },
      // Every session ended on the server; this tab's too.
      { onSuccess: () => void (useSession.getState().status === "authenticated" && signOut()) },
    ),
  );

  const linkError = reset.error instanceof ApiError ? reset.error.fieldErrors.find((e) => e.field === "token")?.message : undefined;
  if (!token || linkError) {
    return (
      <AuthLayout
        title="This link doesn't work"
        subtitle={linkError ?? "The link is incomplete. Open it again from the email."}
      >
        <Button asChild variant="primary" size="lg" className="w-full"><Link to="/forgot-password">Get a new link</Link></Button>
      </AuthLayout>
    );
  }

  if (reset.isSuccess) {
    return (
      <AuthLayout title="Password changed" subtitle="You've been signed out everywhere. Sign in with your new password.">
        <Button asChild variant="primary" size="lg" className="w-full"><Link to="/login">Sign in</Link></Button>
      </AuthLayout>
    );
  }

  const serverPasswordError = reset.error instanceof ApiError ? reset.error.fieldErrors.find((e) => e.field === "password")?.message : undefined;
  return (
    <AuthLayout title="Choose a new password" subtitle="Changing it signs you out on every device.">
      <form onSubmit={onSubmit} noValidate className="space-y-4">
        <FormError message={reset.error && !serverPasswordError ? errorMessage(reset.error) : null} />
        <Field label="New password" error={errors.password?.message ?? serverPasswordError}>
          <Input type="password" autoComplete="new-password" autoFocus {...register("password")} />
        </Field>
        <Field label="Confirm new password" error={errors.confirm?.message}>
          <Input type="password" autoComplete="new-password" {...register("confirm")} />
        </Field>
        <Button type="submit" variant="primary" size="lg" className="w-full" loading={reset.isPending}>
          Change password
        </Button>
      </form>
    </AuthLayout>
  );
}
