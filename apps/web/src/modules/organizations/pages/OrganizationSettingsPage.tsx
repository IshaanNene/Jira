import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";

import { Badge, Button, Card, CardHeader, ErrorState, Field, Input, Page, Skeleton, toast } from "@/components/ui";
import { applyServerErrors, FormError, useMfaStatus } from "@/modules/auth";
import { ApiError, errorMessage, type Organization } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate } from "@/utils/format";

import { useCurrentOrganization, useRenameOrganization, useRequireTwoStep } from "../api";
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
        <div className="space-y-5">
          <GeneralForm organization={organization.data} />
          <TwoStepRequirement organization={organization.data} />
        </div>
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

/** Whether everyone must have two-step verification on to work in this workspace. */
function TwoStepRequirement({ organization }: { organization: Organization }) {
  const role = useSession((s) => s.organization?.role);
  const editable = can(role, "manageOrganization");
  const mfa = useMfaStatus();
  const change = useRequireTwoStep();
  const required = organization.require_two_step;
  // Not a lock-out guard (they could still set it up), just the obvious first step.
  const mineOff = mfa.data !== undefined && !mfa.data.totp_enabled;

  const toggle = () =>
    change.mutate(
      { required: !required, version: organization.version },
      {
        onSuccess: (o) => toast.success(o.require_two_step ? "Two-step verification is now required" : "Two-step verification is no longer required"),
        onError: (e) =>
          toast.error("Couldn't change the setting", e instanceof ApiError && e.isConflict ? "Someone else changed the workspace. Try again." : errorMessage(e)),
      },
    );

  return (
    <Card>
      <CardHeader
        title="Two-step verification"
        description="Require a code from an authenticator app, as well as a password, of everyone who works here."
        actions={required ? <Badge tone="success">Required</Badge> : <Badge>Optional</Badge>}
      />
      <div className="space-y-3 px-4 py-4 text-sm">
        <p className="text-ink-muted">
          {required
            ? "Members without it are asked to set it up before they can open the workspace. API tokens of members without it stop working."
            : "When required, members without it are asked to set it up the next time they open the workspace, within ten minutes for anyone already in."}
        </p>
        {editable ? (
          <div className="flex flex-wrap items-center justify-end gap-3">
            {!required && mineOff && <span className="text-warning">Turn it on for your own account first, in Security settings.</span>}
            <Button size="sm" variant={required ? "secondary" : "primary"} loading={change.isPending} disabled={!required && mineOff} onClick={toggle}>
              {required ? "Stop requiring" : "Require for everyone"}
            </Button>
          </div>
        ) : (
          <p className="text-ink-subtle">Only admins can change workspace settings.</p>
        )}
      </div>
    </Card>
  );
}
