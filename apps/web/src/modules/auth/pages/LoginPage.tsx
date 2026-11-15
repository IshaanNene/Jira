import { zodResolver } from "@hookform/resolvers/zod";
import { useState, type FormEvent } from "react";
import { useForm } from "react-hook-form";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { z } from "zod";

import { Button, Field, Input } from "@/components/ui";
import { ApiError, errorMessage, type MfaChallenge } from "@/services/api";

import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";
import { safeNext } from "../navigation";
import { completeSignIn, signIn } from "../session-service";

const schema = z.object({
  email: z.string().trim().min(1, "Enter your email address.").email("Enter a valid email address."),
  password: z.string().min(1, "Enter your password."),
});
type Values = z.infer<typeof schema>;

export default function LoginPage() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [formError, setFormError] = useState<string | null>(null);
  const [challenge, setChallenge] = useState<MfaChallenge | null>(null);
  const {
    register,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { email: params.get("email") ?? "", password: "" } });

  const done = () => navigate(safeNext(params.get("next")), { replace: true });

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      const next = await signIn(values.email, values.password);
      if (next) setChallenge(next);
      else done();
    } catch (error) {
      setFormError(errorMessage(error));
    }
  });

  if (challenge) {
    return (
      <SecondStep
        challenge={challenge}
        onDone={done}
        onRestart={(message) => {
          setChallenge(null);
          setFormError(message);
        }}
      />
    );
  }

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
        <p className="-mt-2 text-right text-sm">
          <Link to="/forgot-password" className="text-accent hover:underline">
            Forgot your password?
          </Link>
        </p>
        <Button type="submit" variant="primary" size="lg" className="w-full" loading={isSubmitting}>
          Sign in
        </Button>
      </form>
    </AuthLayout>
  );
}

/** The authenticator (or recovery) code after a correct password. */
function SecondStep({ challenge, onDone, onRestart }: { challenge: MfaChallenge; onDone: () => void; onRestart: (message: string | null) => void }) {
  const [recovery, setRecovery] = useState(false);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!code.trim()) {
      setError(recovery ? "Enter one of your recovery codes." : "Enter the code from your authenticator app.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await completeSignIn(challenge.mfa_token, code.trim());
      onDone();
    } catch (err) {
      setBusy(false);
      // 401: the attempt expired or ran out of tries; the password is needed again.
      if (err instanceof ApiError && err.status === 401) onRestart(err.message);
      else setError(err instanceof ApiError && err.fieldErrors[0] ? err.fieldErrors[0].message : errorMessage(err));
    }
  };

  return (
    <AuthLayout
      title="Two-step verification"
      subtitle={recovery ? "Enter one of the recovery codes you saved when you set it up." : "Enter the 6-digit code from your authenticator app."}
      footer={
        <button type="button" className="font-medium text-accent hover:underline" onClick={() => onRestart(null)}>
          Use a different account
        </button>
      }
    >
      <form onSubmit={(e) => void submit(e)} noValidate className="space-y-4">
        <Field label={recovery ? "Recovery code" : "Authentication code"} error={error ?? undefined}>
          {recovery ? (
            <Input key="recovery" autoComplete="off" autoCapitalize="characters" spellCheck={false} autoFocus placeholder="XXXX-XXXX-XXXX"
              value={code} onChange={(e) => setCode(e.target.value)} />
          ) : (
            <Input key="totp" inputMode="numeric" autoComplete="one-time-code" maxLength={7} autoFocus placeholder="123456"
              value={code} onChange={(e) => setCode(e.target.value)} />
          )}
        </Field>
        <Button type="submit" variant="primary" size="lg" className="w-full" loading={busy}>
          Verify
        </Button>
        <button
          type="button"
          className="w-full text-center text-sm text-ink-muted hover:text-ink hover:underline"
          onClick={() => {
            setRecovery((r) => !r);
            setCode("");
            setError(null);
          }}
        >
          {recovery ? "Use your authenticator app instead" : "Can't use your app? Use a recovery code"}
        </button>
      </form>
    </AuthLayout>
  );
}
