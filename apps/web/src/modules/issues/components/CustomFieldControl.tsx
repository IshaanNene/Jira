import { MultiSelect, Select } from "@/components/ui";
import { useUserDirectory } from "@/modules/organizations";
import type { CustomField } from "@/services/api";

import { InlineInput } from "./InlineInput";

const NONE = "__none__";

interface Props {
  field: CustomField;
  value: unknown;
  /** Called with the new value, or `null` to clear. */
  onCommit: (value: unknown) => void;
  disabled?: boolean;
  boxed?: boolean;
}

/** The right editor for a custom field's type. Validation happens on the server. */
export function CustomFieldControl({ field, value, onCommit, disabled, boxed }: Props) {
  const { members } = useUserDirectory();
  const quiet = boxed ? undefined : "border-transparent shadow-none hover:bg-sunken";
  const optional = field.required ? [] : [{ value: NONE, label: "None" }];

  switch (field.field_type) {
    case "select":
      return (
        <Select size="sm" aria-label={field.name} disabled={disabled} className={quiet} value={typeof value === "string" ? value : NONE}
          onValueChange={(v) => onCommit(v === NONE ? null : v)}
          options={[...(typeof value === "string" ? optional : [{ value: NONE, label: "None" }]), ...field.options.map((o) => ({ value: o, label: o }))]} />
      );
    case "multi_select":
      return (
        <MultiSelect aria-label={field.name} disabled={disabled} className={quiet} values={Array.isArray(value) ? (value as string[]) : []}
          onChange={(v) => onCommit(v.length ? v : null)} options={field.options.map((o) => ({ value: o, label: o }))} />
      );
    case "user":
      return (
        <Select size="sm" aria-label={field.name} disabled={disabled} className={quiet} value={typeof value === "string" ? value : NONE}
          onValueChange={(v) => onCommit(v === NONE ? null : v)}
          options={[{ value: NONE, label: "None" }, ...members.filter((m) => m.status === "active" || m.user_id === value).map((m) => ({ value: m.user_id, label: m.display_name }))]} />
      );
    case "checkbox":
      return (
        <label className="flex h-8 items-center gap-2 px-2 text-sm text-ink">
          <input type="checkbox" aria-label={field.name} disabled={disabled} checked={value === true} onChange={(e) => onCommit(e.target.checked)} className="h-4 w-4 accent-[hsl(var(--accent))]" />
          {value === true ? "Yes" : "No"}
        </label>
      );
    case "number":
      return (
        <InlineInput label={field.name} type="number" boxed={boxed} disabled={disabled} placeholder="None" value={typeof value === "number" ? String(value) : ""}
          onSave={(v) => onCommit(v.trim() === "" ? null : Number(v))} />
      );
    case "date":
      return <InlineInput label={field.name} type="date" boxed={boxed} disabled={disabled} value={typeof value === "string" ? value : ""} onSave={(v) => onCommit(v || null)} />;
    case "url":
      return (
        <div className="flex items-center gap-1">
          <InlineInput label={field.name} type="url" boxed={boxed} disabled={disabled} placeholder="https://…" value={typeof value === "string" ? value : ""} onSave={(v) => onCommit(v.trim() || null)} />
          {typeof value === "string" && /^https?:\/\//i.test(value) && (
            <a href={value} target="_blank" rel="noopener noreferrer nofollow" className="shrink-0 text-xs font-medium text-accent hover:underline">Open</a>
          )}
        </div>
      );
    default:
      return <InlineInput label={field.name} boxed={boxed} disabled={disabled} placeholder="None" value={typeof value === "string" ? value : ""} onSave={(v) => onCommit(v.trim() || null)} />;
  }
}
