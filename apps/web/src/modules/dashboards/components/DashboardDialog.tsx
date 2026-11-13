import { useState, type FormEvent, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";

import { Button, Dialog, DialogContent, DialogFooter, DialogTrigger, Field, Input, Textarea } from "@/components/ui";
import { FormError } from "@/modules/auth";
import { errorMessage, type Dashboard } from "@/services/api";

import { useCreateDashboard, useUpdateDashboard } from "../api";

/** Create a dashboard, or rename one (`dashboard` given). */
export function DashboardDialog({ dashboard, trigger }: { dashboard?: Dashboard; trigger: ReactNode }) {
  const navigate = useNavigate();
  const create = useCreateDashboard();
  const update = useUpdateDashboard(dashboard?.id ?? "");
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (next) {
      setName(dashboard?.name ?? "");
      setDescription(dashboard?.description ?? "");
      setError(null);
    }
  };

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return setError("Give the dashboard a name.");
    setError(null);
    try {
      if (dashboard) {
        await update.mutateAsync({ name: trimmed, description: description.trim() });
        setOpen(false);
      } else {
        const created = await create.mutateAsync({ name: trimmed, description: description.trim() });
        setOpen(false);
        navigate(`/dashboards/${created.id}`);
      }
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent title={dashboard ? "Edit dashboard" : "Create dashboard"} description={dashboard ? undefined : "Dashboards start private. Share one to let everyone in the workspace see it."}>
        <form onSubmit={(e) => void onSubmit(e)} noValidate className="space-y-4">
          <FormError message={error} />
          <Field label="Name">
            <Input autoFocus maxLength={80} placeholder="Team health" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Description">
            <Textarea maxLength={500} rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
          </Field>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setOpen(false)}>Cancel</Button>
            <Button type="submit" variant="primary" loading={create.isPending || update.isPending}>{dashboard ? "Save" : "Create"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
