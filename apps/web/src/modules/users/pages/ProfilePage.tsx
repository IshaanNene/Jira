import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";

import { Avatar, Button, Card, CardHeader, ErrorState, Field, Input, Page, Select, Skeleton, toast } from "@/components/ui";
import { applyServerErrors, ChangeEmailDialog, FormError } from "@/modules/auth";
import type { Profile } from "@/services/api";

import { useMyProfile, useUpdateProfile } from "../api";

const schema = z.object({
  display_name: z.string().trim().min(1, "Enter your name.").max(80, "Use 80 characters or fewer."),
  timezone: z.string().min(1),
  locale: z.string().min(2),
});
type Values = z.infer<typeof schema>;

const LOCALES = [
  { value: "en", label: "English" },
  { value: "de", label: "Deutsch" },
  { value: "es", label: "Español" },
  { value: "fr", label: "Français" },
  { value: "ja", label: "日本語" },
  { value: "pt-BR", label: "Português (Brasil)" },
];

function timezones(current: string): { value: string; label: string }[] {
  const supported = typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf("timeZone") : [];
  const all = new Set(["UTC", current, ...supported]);
  return [...all].map((tz) => ({ value: tz, label: tz.replace(/_/g, " ") }));
}

export default function ProfilePage() {
  const profile = useMyProfile();
  return (
    <Page title="Profile" description="How you appear to the people you work with." width="narrow">
      {profile.isPending ? (
        <Card className="space-y-3 p-4"><Skeleton className="h-10 w-10 rounded-full" /><Skeleton className="h-9" /><Skeleton className="h-9" /></Card>
      ) : profile.isError ? (
        <Card><ErrorState error={profile.error} onRetry={() => void profile.refetch()} /></Card>
      ) : (
        <ProfileForm profile={profile.data} />
      )}
    </Page>
  );
}

function ProfileForm({ profile }: { profile: Profile }) {
  const update = useUpdateProfile();
  const [formError, setFormError] = useState<string | null>(null);
  const {
    register,
    control,
    handleSubmit,
    reset,
    setError,
    watch,
    formState: { errors, isSubmitting, isDirty },
  } = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: { display_name: profile.display_name, timezone: profile.timezone, locale: profile.locale },
  });

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      const saved = await update.mutateAsync(values);
      reset({ display_name: saved.display_name, timezone: saved.timezone, locale: saved.locale });
      toast.success("Profile saved");
    } catch (error) {
      setFormError(applyServerErrors(error, setError, ["display_name", "timezone", "locale"]));
    }
  });

  const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  return (
    <Card>
      <CardHeader title="Personal details" />
      <form onSubmit={onSubmit} noValidate className="space-y-4 p-4">
        <FormError message={formError} />
        <div className="flex items-center gap-3">
          <Avatar name={watch("display_name") || profile.display_name} seed={profile.user_id} size="lg" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-base font-medium text-ink">{profile.email}</div>
            <div className="text-sm text-ink-subtle">Your sign-in email.</div>
          </div>
          <ChangeEmailDialog current={profile.email} />
        </div>
        <Field label="Full name" error={errors.display_name?.message}>
          <Input autoComplete="name" {...register("display_name")} />
        </Field>
        <div className="grid gap-4 sm:grid-cols-2">
          <Controller
            control={control}
            name="timezone"
            render={({ field }) => (
              <Field label="Time zone" hint={browserZone !== field.value ? `Your browser is set to ${browserZone.replace(/_/g, " ")}.` : undefined}>
                <Select value={field.value} onValueChange={field.onChange} options={timezones(profile.timezone)} />
              </Field>
            )}
          />
          <Controller
            control={control}
            name="locale"
            render={({ field }) => (
              <Field label="Language">
                <Select value={field.value} onValueChange={field.onChange} options={LOCALES.some((l) => l.value === field.value) ? LOCALES : [...LOCALES, { value: field.value, label: field.value }]} />
              </Field>
            )}
          />
        </div>
        <div className="flex justify-end">
          <Button type="submit" variant="primary" loading={isSubmitting} disabled={!isDirty}>
            Save changes
          </Button>
        </div>
      </form>
    </Card>
  );
}
