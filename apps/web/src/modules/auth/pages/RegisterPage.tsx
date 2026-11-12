import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { z } from "zod";

import { Button, Field, Input } from "@/components/ui";

import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";
import { applyServerErrors } from "../form-errors";
import { safeNext } from "../navigation";
import { signUp } from "../session-service";

// Mirrors the server's rules for immediate feedback; the server remains the authority.
const schema = z.object({
  display_name: z.string().trim().min(1, "Enter your name.").max(80, "Name must be 80 characters or fewer."),
  email: z.string().trim().min(1, "Enter your email address.").email("Enter a valid email address."),
  password: z.string().min(10, "Use at least 10 characters.").max(128, "Use 128 characters or fewer."),
});
type Values = z.infer<typeof schema>;

export default function RegisterPage() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [formError, setFormError] = useState<string | null>(null);
  const {
    register,
    handleSubmit,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: { display_name: "", email: params.get("email") ?? "", password: "" },
  });

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      await signUp(values);
      navigate(safeNext(params.get("next")), { replace: true });
    } catch (error) {
      setFormError(applyServerErrors(error, setError, ["display_name", "email", "password"]));
    }
  });

  const loginHref = `/login${params.toString() ? `?${params.toString()}` : ""}`;
  return (
    <AuthLayout
      title="Create your account"
      subtitle="Set up your account, then create or join a workspace."
      footer={
        <>
          Already have an account?{" "}
          <Link to={loginHref} className="font-medium text-accent hover:underline">
            Sign in
          </Link>
        </>
      }
    >
      <form onSubmit={onSubmit} noValidate className="space-y-4">
        <FormError message={formError} />
        <Field label="Full name" error={errors.display_name?.message}>
          <Input autoComplete="name" autoFocus {...register("display_name")} />
        </Field>
        <Field label="Work email" error={errors.email?.message}>
          <Input type="email" autoComplete="email" {...register("email")} />
        </Field>
        <Field label="Password" error={errors.password?.message} hint="At least 10 characters.">
          <Input type="password" autoComplete="new-password" {...register("password")} />
        </Field>
        <Button type="submit" variant="primary" size="lg" className="w-full" loading={isSubmitting}>
          Create account
        </Button>
      </form>
    </AuthLayout>
  );
}
