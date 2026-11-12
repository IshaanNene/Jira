import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { z } from "zod";

import { Button, Field, Input } from "@/components/ui";
import { errorMessage } from "@/services/api";

import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";
import { safeNext } from "../navigation";
import { signIn } from "../session-service";

const schema = z.object({
  email: z.string().trim().min(1, "Enter your email address.").email("Enter a valid email address."),
  password: z.string().min(1, "Enter your password."),
});
type Values = z.infer<typeof schema>;

export default function LoginPage() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [formError, setFormError] = useState<string | null>(null);
  const {
    register,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { email: params.get("email") ?? "", password: "" } });

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      await signIn(values.email, values.password);
      navigate(safeNext(params.get("next")), { replace: true });
    } catch (error) {
      setFormError(errorMessage(error));
    }
  });

  const registerHref = `/register${params.toString() ? `?${params.toString()}` : ""}`;
  return (
    <AuthLayout
      title="Sign in"
      subtitle="Welcome back. Enter your details to continue."
      footer={
        <>
          New here?{" "}
          <Link to={registerHref} className="font-medium text-accent hover:underline">
            Create an account
          </Link>
        </>
      }
    >
      <form onSubmit={onSubmit} noValidate className="space-y-4">
        <FormError message={formError} />
        <Field label="Email" error={errors.email?.message}>
          <Input type="email" autoComplete="email" autoFocus {...register("email")} />
        </Field>
        <Field label="Password" error={errors.password?.message}>
          <Input type="password" autoComplete="current-password" {...register("password")} />
        </Field>
        <Button type="submit" variant="primary" size="lg" className="w-full" loading={isSubmitting}>
          Sign in
        </Button>
      </form>
    </AuthLayout>
  );
}
