import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";

import { Button, Field, Input } from "@/components/ui";
import { applyServerErrors, FormError } from "@/modules/auth";

import { useCreateOrganization } from "../api";

const schema = z.object({
  name: z.string().trim().min(2, "Use at least 2 characters.").max(80, "Use 80 characters or fewer."),
  slug: z
    .string()
    .trim()
    .toLowerCase()
    .regex(/^$|^[a-z0-9]+(-[a-z0-9]+)*$/, "Use lowercase letters, numbers and single hyphens.")
    .refine((v) => v === "" || (v.length >= 3 && v.length <= 40), "Use between 3 and 40 characters."),
});
type Values = z.infer<typeof schema>;

/** Mirrors the server's slug derivation so the preview matches what will be created. */
export function slugPreview(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/g, "");
  return slug.padEnd(Math.min(3, slug.length === 0 ? 0 : 3), "0");
}

export function CreateOrganizationForm({ onCreated, submitLabel = "Create workspace" }: { onCreated: (organizationId: string) => Promise<void> | void; submitLabel?: string }) {
  const create = useCreateOrganization();
  const [formError, setFormError] = useState<string | null>(null);
  const {
    register,
    handleSubmit,
    setError,
    watch,
    formState: { errors, isSubmitting },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { name: "", slug: "" } });
  const derived = slugPreview(watch("name"));

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      const organization = await create.mutateAsync({ name: values.name, slug: values.slug || undefined });
      await onCreated(organization.id);
    } catch (error) {
      setFormError(applyServerErrors(error, setError, ["name", "slug"]));
    }
  });

  return (
    <form onSubmit={onSubmit} noValidate className="space-y-4">
      <FormError message={formError} />
      <Field label="Workspace name" error={errors.name?.message} hint="Usually your company or team name.">
        <Input autoFocus autoComplete="organization" placeholder="Acme Inc." {...register("name")} />
      </Field>
      <Field label="URL name" error={errors.slug?.message} hint="Optional. Leave blank to use the suggestion.">
        <Input placeholder={derived || "acme-inc"} autoCapitalize="none" spellCheck={false} {...register("slug")} />
      </Field>
      <Button type="submit" variant="primary" size="lg" className="w-full" loading={isSubmitting}>
        {submitLabel}
      </Button>
    </form>
  );
}
