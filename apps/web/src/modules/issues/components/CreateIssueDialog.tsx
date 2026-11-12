import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useNavigate } from "react-router-dom";
import { z } from "zod";

import { Button, Dialog, DialogContent, DialogFooter, Field, Input, Select, toast } from "@/components/ui";
import { applyServerErrors, FormError } from "@/modules/auth";
import { useUserDirectory } from "@/modules/organizations";
import { roleAtLeast, useProjects } from "@/modules/projects";
import type { RichDoc } from "@/services/api";
import { useUi } from "@/state/ui";
import { titleCase } from "@/utils/format";

import { applicableFields, useCreateIssue, useCustomFields, useIssueMeta } from "../api";
import { CustomFieldControl } from "./CustomFieldControl";
import { isEmptyDoc, RichTextEditor } from "./RichText";

const schema = z.object({
  project_key: z.string().min(1, "Choose a project."),
  type: z.string().min(1),
  summary: z.string().trim().min(1, "Enter a summary.").max(255, "Use 255 characters or fewer."),
  priority: z.string(),
  assignee_id: z.string(),
  labels: z.string(),
});
type Values = z.infer<typeof schema>;
const UNASSIGNED = "unassigned";

/** The global "Create" dialog. Opened from the top bar, the `c` shortcut, or a parent issue. */
export function CreateIssueDialog() {
  const navigate = useNavigate();
  const request = useUi((s) => s.createIssue);
  const close = useUi((s) => s.closeCreateIssue);
  const projects = useProjects();
  const meta = useIssueMeta();
  const { members } = useUserDirectory();
  const create = useCreateIssue();
  const [description, setDescription] = useState<RichDoc | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [custom, setCustom] = useState<Record<string, unknown>>({});
  const customFields = useCustomFields();
  const writable = (projects.data ?? []).filter((p) => roleAtLeast(p.my_role, "member"));
  const { register, control, handleSubmit, reset, setError, watch, formState: { errors, isSubmitting } } = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: { project_key: "", type: "task", summary: "", priority: "medium", assignee_id: UNASSIGNED, labels: "" },
  });

  const parentKey = request?.parentKey;
  const open = request !== null;
  useEffect(() => {
    if (!open) return;
    setDescription(null);
    setFormError(null);
    setCustom({});
    reset({
      project_key: request?.projectKey ?? writable[0]?.key ?? "",
      type: parentKey ? "subtask" : "task",
      summary: "",
      priority: "medium",
      assignee_id: UNASSIGNED,
      labels: "",
    });
    // Runs when the dialog opens; the defaults are a snapshot of that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, projects.isSuccess]);

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      const issue = await create.mutateAsync({
        project_key: values.project_key,
        type: values.type,
        summary: values.summary,
        priority: values.priority,
        assignee_id: values.assignee_id === UNASSIGNED ? null : values.assignee_id,
        labels: values.labels.split(/[\s,]+/).filter(Boolean),
        description: isEmptyDoc(description) ? null : description,
        parent_key: parentKey ?? null,
        // Only fields that apply to the chosen project and type are sent.
        custom_fields: Object.fromEntries(fields.filter((f) => custom[f.id] !== undefined && custom[f.id] !== null).map((f) => [f.id, custom[f.id]])),
      });
      close();
      toast.success(`${issue.key} created`, issue.summary);
      if (!parentKey) navigate(`/projects/${issue.project_key}/issues/${issue.key}`);
    } catch (error) {
      setFormError(applyServerErrors(error, setError, ["project_key", "type", "summary", "priority", "assignee_id", "labels"]));
    }
  });

  // A child issue's type is constrained by its parent; the server decides, the form guides.
  const fields = applicableFields(customFields.data, watch("project_key"), watch("type"));
  const types = (meta.data?.types ?? []).filter((t) => (parentKey ? t.level <= 0 : t.level >= 0));

  return (
    <Dialog open={open} onOpenChange={(next) => !next && close()}>
      <DialogContent title={parentKey ? `Create child issue of ${parentKey}` : "Create issue"} className="top-[6%] max-h-[88vh] max-w-2xl overflow-y-auto">
        <form onSubmit={onSubmit} noValidate className="space-y-4">
          <FormError message={formError} />
          <div className="grid gap-4 sm:grid-cols-2">
            <Controller control={control} name="project_key" render={({ field }) => (
              <Field label="Project" error={errors.project_key?.message}>
                <Select value={field.value || "none"} disabled={Boolean(parentKey)} onValueChange={field.onChange}
                  options={writable.length ? writable.map((p) => ({ value: p.key, label: `${p.name} (${p.key})` })) : [{ value: "none", label: projects.isPending ? "Loading…" : "No projects you can create issues in" }]} />
              </Field>
            )} />
            <Controller control={control} name="type" render={({ field }) => (
              <Field label="Issue type"><Select value={field.value} onValueChange={field.onChange} options={types.map((t) => ({ value: t.key, label: t.name }))} /></Field>
            )} />
          </div>
          <Field label="Summary" error={errors.summary?.message}>
            <Input autoFocus maxLength={255} {...register("summary")} />
          </Field>
          <div>
            <div className="mb-1.5 text-sm font-medium text-ink">Description</div>
            <RichTextEditor aria-label="Description" placeholder="Add more detail…" onChange={setDescription} minHeight="8rem" />
          </div>
          <div className="grid gap-4 sm:grid-cols-2">
            <Controller control={control} name="assignee_id" render={({ field }) => (
              <Field label="Assignee">
                <Select value={field.value} onValueChange={field.onChange} options={[{ value: UNASSIGNED, label: "Unassigned" }, ...members.filter((m) => m.status === "active").map((m) => ({ value: m.user_id, label: m.display_name }))]} />
              </Field>
            )} />
            <Controller control={control} name="priority" render={({ field }) => (
              <Field label="Priority"><Select value={field.value} onValueChange={field.onChange} options={(meta.data?.priorities ?? ["medium"]).map((p) => ({ value: p, label: titleCase(p) }))} /></Field>
            )} />
          </div>
          <Field label="Labels" error={errors.labels?.message} hint="Separate with spaces or commas.">
            <Input placeholder="backend api" {...register("labels")} />
          </Field>
          {fields.length > 0 && (
            <div className="grid gap-4 sm:grid-cols-2">
              {fields.map((field) => (
                <div key={field.id} className="space-y-1.5">
                  <div className="text-sm font-medium text-ink">{field.name}{field.required && <span className="text-danger" aria-label="required"> *</span>}</div>
                  <CustomFieldControl boxed field={field} value={custom[field.id]} onCommit={(value) => setCustom((c) => ({ ...c, [field.id]: value }))} />
                </div>
              ))}
            </div>
          )}
          <DialogFooter>
            <Button onClick={close}>Cancel</Button>
            <Button type="submit" variant="primary" loading={isSubmitting} disabled={writable.length === 0}>Create</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
