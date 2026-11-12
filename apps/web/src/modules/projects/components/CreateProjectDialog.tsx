import { zodResolver } from "@hookform/resolvers/zod";
import { useState, type ReactNode } from "react";
import { Controller, useForm } from "react-hook-form";
import { useNavigate } from "react-router-dom";
import { z } from "zod";

import { Button, Dialog, DialogContent, DialogFooter, DialogTrigger, Field, Input, Select } from "@/components/ui";
import { applyServerErrors, FormError } from "@/modules/auth";

import { useCreateProject } from "../api";
import { suggestKey } from "../roles";

const schema = z.object({
  name: z.string().trim().min(2, "Use at least 2 characters.").max(80, "Use 80 characters or fewer."),
  key: z.string().trim().toUpperCase().regex(/^[A-Z][A-Z0-9]{1,9}$/, "Use 2–10 letters or digits, starting with a letter."),
  project_type: z.enum(["scrum", "kanban", "general"]),
  access: z.enum(["open", "private"]),
});
type Values = z.infer<typeof schema>;

const TYPES = [
  { value: "scrum", label: "Scrum", description: "Plan work in sprints with a backlog" },
  { value: "kanban", label: "Kanban", description: "Continuous flow of work on a board" },
  { value: "general", label: "General", description: "Simple task tracking" },
];
const ACCESS = [
  { value: "open", label: "Open", description: "Everyone in the workspace can work in it" },
  { value: "private", label: "Private", description: "Only people you add can see it" },
];

export function CreateProjectDialog({ trigger }: { trigger: ReactNode }) {
  const navigate = useNavigate();
  const create = useCreateProject();
  const [open, setOpen] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const {
    register,
    control,
    handleSubmit,
    setError,
    setValue,
    reset,
    formState: { errors, isSubmitting, dirtyFields },
  } = useForm<Values>({ resolver: zodResolver(schema), defaultValues: { name: "", key: "", project_type: "scrum", access: "open" } });

  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) {
      reset();
      setFormError(null);
    }
  };

  const onSubmit = handleSubmit(async (values) => {
    setFormError(null);
    try {
      const project = await create.mutateAsync({ ...values, description: "" });
      onOpenChange(false);
      navigate(`/projects/${project.key}/issues`);
    } catch (error) {
      setFormError(applyServerErrors(error, setError, ["name", "key", "project_type", "access"]));
    }
  });

  const name = register("name");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent title="Create project" description="A project groups related issues and the people working on them.">
        <form onSubmit={onSubmit} noValidate className="space-y-4">
          <FormError message={formError} />
          <Field label="Name" error={errors.name?.message}>
            <Input
              autoFocus
              placeholder="Payments platform"
              {...name}
              onChange={(e) => {
                void name.onChange(e);
                // Keep suggesting a key until the person types their own.
                if (!dirtyFields.key) setValue("key", suggestKey(e.target.value));
              }}
            />
          </Field>
          <Field label="Key" error={errors.key?.message} hint="Prefix of every issue in this project (e.g. PAY-12). It cannot be changed later.">
            <Input className="w-40 font-mono uppercase" maxLength={10} autoCapitalize="characters" spellCheck={false} {...register("key")} />
          </Field>
          <div className="grid gap-4 sm:grid-cols-2">
            <Controller control={control} name="project_type" render={({ field }) => (
              <Field label="Type"><Select value={field.value} onValueChange={field.onChange} options={TYPES} /></Field>
            )} />
            <Controller control={control} name="access" render={({ field }) => (
              <Field label="Access"><Select value={field.value} onValueChange={field.onChange} options={ACCESS} /></Field>
            )} />
          </div>
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="primary" loading={isSubmitting}>Create project</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
