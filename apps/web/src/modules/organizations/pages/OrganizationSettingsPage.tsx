import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";

import { Button, Card, CardHeader, ErrorState, Field, Input, Page, Skeleton, toast } from "@/components/ui";
import { applyServerErrors, FormError } from "@/modules/auth";
import { ApiError, type Organization } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate } from "@/utils/format";

import { useCurrentOrganization, useRenameOrganization } from "../api";
import { can } from "../permissions";

const schema = z.object({ name: z.string().trim().min(2, "Use at least 2 characters.").max(80, "Use 80 characters or fewer.") });
type Values = z.infer<typeof schema>;

export default function OrganizationSettingsPage() {
  const organization = useCurrentOrganization();
  return (
    <Page title="Workspace settings" description="Details that identify this workspace to its members." width="narrow">
      {organization.isPending ? (
        <Card className="space-y-3 p-4"><Skeleton className="h-4 w-32" /><Skeleton className="h-9" /><Skeleton className="h-9 w-24" /></Card>
      ) : organization.isError ? (
        <Card><ErrorState error={organization.error} onRetry={() => void organization.refetch()} /></Card>
      ) : (
        <GeneralForm organization={organization.data} />
      )}
    </Page>
  );
}

function GeneralForm({ organization }: { organization: Organization }) {
  const role = useSession((s) => s.organization?.role);
  const editable = can(role, "manageOrganization");
  const rename = useRenameOrganization();
  const [formError, setFormError] = useState<string | null>(null);
  // The version this form's edits are based on. Sent as If-Match so a concurrent edit
  // by someone else is detected instead of silently overwritten.
  const [baseVersion, setBaseVersion] = useState(organization.version);
  const [conflict, setConflict] = useState<Organization | null>(null);
  const {
    register,
    handleSubmit,
    reset,
    setError,
    formState: { errors, isSubmitting, isDirty },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { name: organization.name } });

  // Pick up changes made elsewhere while this form is untouched.
  useEffect(() => {
    if (!isDirty && organization.version !== baseVersion) {
      setBaseVersion(organization.version);
      reset({ name: organization.name });
    }
  }, [organization, baseVersion, isDirty, reset]);

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    setConflict(null);
    try {
      const updated = await rename.mutateAsync({ name: values.name, version: baseVersion });
      setBaseVersion(updated.version);
      reset({ name: updated.name });
      toast.success("Workspace updated");
    } catch (error) {
      if (error instanceof ApiError && error.isConflict && error.current) {
        setConflict(error.current as Organization);
      } else {
        setFormError(applyServerErrors(error, setError, ["name"]));
      }
    }
  });

  return (
    <Card>
      <CardHeader title="General" description={`Created ${formatDate(organization.created_at)}`} />
      <form onSubmit={onSubmit} noValidate className="space-y-4 p-4">
        <FormError message={formError} />
        {conflict && (
          <div role="alert" className="rounded border border-warning/30 bg-warning-soft px-3 py-2 text-sm text-warning">
            <p>Someone else renamed this workspace to “{conflict.name}” while you were editing.</p>
            <div className="mt-2 flex gap-2">
              <Button size="sm" onClick={() => { setBaseVersion(conflict.version); setConflict(null); void onSubmit(); }}>
                Keep my change
              </Button>
              <Button size="sm" variant="ghost" onClick={() => { setBaseVersion(conflict.version); reset({ name: conflict.name }); setConflict(null); }}>
                Use theirs
              </Button>
            </div>
          </div>
        )}
        <Field label="Workspace name" error={errors.name?.message} hint={editable ? undefined : "Only admins can change workspace settings."}>
          <Input disabled={!editable} {...register("name")} />
        </Field>
        <Field label="URL name" hint="Used in links. It cannot be changed yet.">
          <Input value={organization.slug} readOnly disabled />
        </Field>
        {editable && (
          <div className="flex justify-end">
            <Button type="submit" variant="primary" loading={isSubmitting} disabled={!isDirty}>
              Save changes
            </Button>
          </div>
        )}
      </form>
    </Card>
  );
}
