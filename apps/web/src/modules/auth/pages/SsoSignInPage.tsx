import { useState, type FormEvent } from "react";
import { Link, useSearchParams } from "react-router-dom";

import { Button, Field, Input } from "@/components/ui";
import { ApiError, errorMessage } from "@/services/api";

import { AuthLayout } from "../components/AuthLayout";
import { FormError } from "../components/FormError";
import { startSso } from "../session-service";

/** "Sign in with SSO": the workspace's sign-in name leads to its identity provider. */
export default function SsoSignInPage() {
  const [params] = useSearchParams();
  const [name, setName] = useState(params.get("name") ?? "");
  const [error, setError] = useState<string | null>(null);
  const [fieldError, setFieldError] = useState<string | undefined>();
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim()) {
      setFieldError("Enter your workspace's sign-in name.");
      return;
    }
    setBusy(true);
    setError(null);
    setFieldError(undefined);
    try {
      await startSso(name.trim());
      // The browser is on its way to the identity provider.
    } catch (err) {
      setBusy(false);
      if (err instanceof ApiError && err.fieldErrors[0]) setFieldError(err.fieldErrors[0].message);
      else setError(errorMessage(err));
    }
  };

  return (
    <AuthLayout
      title="Sign in with SSO"
      subtitle="Your workspace admin can tell you its sign-in name."
      footer={
        <Link to="/login" className="font-medium text-accent hover:underline">
          Sign in another way
        </Link>
      }
    >
      <form onSubmit={(e) => void submit(e)} noValidate className="space-y-4">
        <FormError message={error} />
        <Field label="Workspace sign-in name" error={fieldError}>
          <Input autoFocus autoCapitalize="none" spellCheck={false} placeholder="acme" value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Button type="submit" variant="primary" size="lg" className="w-full" loading={busy}>
          Continue
        </Button>
      </form>
    </AuthLayout>
  );
}
