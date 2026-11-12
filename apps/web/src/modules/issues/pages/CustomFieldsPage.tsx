import { Lock, SlidersHorizontal } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";

import { Badge, Button, Card, Dialog, DialogContent, DialogFooter, EmptyState, ErrorState, Field, Input, MultiSelect, Page, Select, Skeleton, Textarea, toast } from "@/components/ui";
import { FormError } from "@/modules/auth";
import { can } from "@/modules/organizations";
import { useProjects } from "@/modules/projects";
import { ApiError, errorMessage, type CustomField } from "@/services/api";
import { useSession } from "@/state/session";

import { useCustomFieldMutations, useCustomFields, useIssueMeta, type CustomFieldInput } from "../api";

const TYPE_LABEL: Record<string, string> = {
  text: "Text",
  number: "Number",
  date: "Date",
  select: "Dropdown (single choice)",
  multi_select: "Dropdown (multiple choices)",
  checkbox: "Checkbox",
  url: "Link",
  user: "Person",
};
const hasOptions = (type: string) => type === "select" || type === "multi_select";

export default function CustomFieldsPage() {
  const role = useSession((s) => s.organization?.role);
  const fields = useCustomFields();
  const { archive } = useCustomFieldMutations();
  // `null` closed, "new" creating, otherwise the field being edited.
  const [editing, setEditing] = useState<CustomField | "new" | null>(null);

  if (!can(role, "manageOrganization")) {
    return (
      <Page title="Custom fields" width="narrow">
        <EmptyState icon={Lock} title="Only workspace admins manage custom fields" description="Ask an admin if an issue needs a field that doesn't exist yet." />
      </Page>
    );
  }
  const scope = (f: CustomField) =>
    [f.project_keys.length ? f.project_keys.join(", ") : "All projects", f.issue_types.length ? f.issue_types.join(", ") : "all issue types"].join(" · ");

  return (
    <Page title="Custom fields" description="Extra fields on issues, for the projects and issue types you choose." actions={<Button variant="primary" onClick={() => setEditing("new")}>Create field</Button>}>
      <Card>
        {fields.isPending ? (
          <div className="space-y-2 p-4" aria-busy><Skeleton className="h-10" /><Skeleton className="h-10" /></div>
        ) : fields.isError ? (
          <ErrorState error={fields.error} onRetry={() => void fields.refetch()} />
        ) : fields.data.length === 0 ? (
          <EmptyState icon={SlidersHorizontal} title="No custom fields yet" description="Create a field to capture information the built-in fields don't cover, such as Severity or Customer." action={<Button variant="primary" onClick={() => setEditing("new")}>Create field</Button>} />
        ) : (
          <ul className="divide-y divide-line">
            {fields.data.map((field) => (
              <li key={field.id} className="flex items-center gap-3 px-4 py-2.5">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-base font-medium text-ink">{field.name}</span>
                    {field.required && <Badge tone="warning">Required</Badge>}
                  </div>
                  <div className="truncate text-sm text-ink-muted">{TYPE_LABEL[field.field_type] ?? field.field_type} · {scope(field)}</div>
                </div>
                <Button size="sm" onClick={() => setEditing(field)}>Edit</Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    if (!window.confirm(`Remove “${field.name}”? It disappears from issues; the values already entered are kept in case you need them back.`)) return;
                    archive.mutate(field.id, { onSuccess: () => toast.success("Field removed"), onError: (e) => toast.error("Couldn't remove field", errorMessage(e)) });
                  }}
                >
                  Remove
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <FieldDialog editing={editing} onClose={() => setEditing(null)} />
    </Page>
  );
}

function FieldDialog({ editing, onClose }: { editing: CustomField | "new" | null; onClose: () => void }) {
  const projects = useProjects();
  const meta = useIssueMeta();
  const { create, update } = useCustomFieldMutations();
  const existing = editing && editing !== "new" ? editing : null;
  const [form, setForm] = useState<CustomFieldInput>({ name: "", field_type: "text", options: [], required: false, project_keys: [], issue_types: [] });
  const [optionsText, setOptionsText] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!editing) return;
    setError(null);
    setForm(existing ? { name: existing.name, field_type: existing.field_type, options: existing.options, required: existing.required, project_keys: existing.project_keys, issue_types: existing.issue_types } : { name: "", field_type: "text", options: [], required: false, project_keys: [], issue_types: [] });
    setOptionsText(existing?.options.join("\n") ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    const input = { ...form, name: form.name.trim(), options: hasOptions(form.field_type) ? optionsText.split("\n").map((o) => o.trim()).filter(Boolean) : [] };
    const done = { onSuccess: () => { toast.success(existing ? "Field updated" : "Field created"); onClose(); }, onError: (err: unknown) => setError(err instanceof ApiError && err.fieldErrors[0] ? err.fieldErrors[0].message : errorMessage(err)) };
    if (existing) update.mutate({ ...input, id: existing.id }, done);
    else create.mutate(input, done);
  };

  return (
    <Dialog open={editing !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={existing ? `Edit ${existing.name}` : "Create custom field"} className="max-w-lg">
        <form onSubmit={submit} noValidate className="space-y-4">
          <FormError message={error} />
          <Field label="Name"><Input autoFocus maxLength={60} value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} /></Field>
          <Field label="Type" hint={existing ? "The type can't be changed after creation." : undefined}>
            <Select value={form.field_type} disabled={Boolean(existing)} onValueChange={(v) => setForm({ ...form, field_type: v })} options={Object.entries(TYPE_LABEL).map(([value, label]) => ({ value, label }))} />
          </Field>
          {hasOptions(form.field_type) && (
            <Field label="Options" hint="One per line."><Textarea rows={4} value={optionsText} onChange={(e) => setOptionsText(e.target.value)} /></Field>
          )}
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-1.5">
              <div className="text-sm font-medium text-ink">Projects</div>
              <MultiSelect aria-label="Projects" placeholder="All projects" values={form.project_keys} onChange={(v) => setForm({ ...form, project_keys: v })} options={(projects.data ?? []).map((p) => ({ value: p.key, label: `${p.name} (${p.key})` }))} />
            </div>
            <div className="space-y-1.5">
              <div className="text-sm font-medium text-ink">Issue types</div>
              <MultiSelect aria-label="Issue types" placeholder="All issue types" values={form.issue_types} onChange={(v) => setForm({ ...form, issue_types: v })} options={(meta.data?.types ?? []).map((t) => ({ value: t.key, label: t.name }))} />
            </div>
          </div>
          <label className="flex items-center gap-2 text-sm text-ink">
            <input type="checkbox" checked={form.required} onChange={(e) => setForm({ ...form, required: e.target.checked })} className="h-4 w-4 accent-[hsl(var(--accent))]" />
            Required when creating an issue
          </label>
          <DialogFooter>
            <Button onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" loading={create.isPending || update.isPending} disabled={!form.name.trim()}>{existing ? "Save" : "Create field"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
