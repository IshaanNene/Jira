import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Link, useSearchParams } from "react-router-dom";
import { z } from "zod";

import { Button, Field, Input } from "@/components/ui";
import { errorMessage } from "@/services/api";

import { useForgotPassword } from "../api";
import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";

const schema = z.object({ email: z.string().trim().min(1, "Enter your email address.").email("Enter a valid email address.") });
type Values = z.infer<typeof schema>;

export default function ForgotPasswordPage() {
  const [params] = useSearchParams();
  const forgot = useForgotPassword();
  const [sentTo, setSentTo] = useState<string | null>(null);
  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { email: params.get("email") ?? "" } });

  const onSubmit = handleSubmit((values) => forgot.mutate(values.email, { onSuccess: () => setSentTo(values.email) }));
  const back = (
    <Link to="/login" className="font-medium text-accent hover:underline">
      Back to sign in
    </Link>
  );

  if (sentTo) {
    return (
      <AuthLayout
        title="Check your email"
        subtitle={<>If <strong className="font-medium text-ink">{sentTo}</strong> has an account, we've sent it a link to choose a new password. The link expires in an hour.</>}
        footer={back}
      >
        <Button size="lg" className="w-full" onClick={() => setSentTo(null)}>Use a different address</Button>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout title="Reset your password" subtitle="Enter your account's email address and we'll send you a link to choose a new password." footer={back}>
      <form onSubmit={onSubmit} noValidate className="space-y-4">
        <FormError message={forgot.error ? errorMessage(forgot.error) : null} />
        <Field label="Email" error={errors.email?.message}>
          <Input type="email" autoComplete="email" autoFocus {...register("email")} />
        </Field>
        <Button type="submit" variant="primary" size="lg" className="w-full" loading={forgot.isPending}>
          Send reset link
        </Button>
      </form>
    </AuthLayout>
  );
}
