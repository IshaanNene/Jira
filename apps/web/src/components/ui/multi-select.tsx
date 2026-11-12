import * as Menu from "@radix-ui/react-dropdown-menu";
import { Check, ChevronDown } from "lucide-react";

import { cn } from "@/utils/cn";

interface MultiSelectProps {
  values: string[];
  onChange: (values: string[]) => void;
  options: { value: string; label: string }[];
  placeholder?: string;
  disabled?: boolean;
  "aria-label": string;
  className?: string;
}

/** Choose any number of options. The menu stays open while toggling. */
export function MultiSelect({ values, onChange, options, placeholder = "None", disabled, className, ...aria }: MultiSelectProps) {
  const selected = options.filter((o) => values.includes(o.value));
  const toggle = (value: string) => onChange(values.includes(value) ? values.filter((v) => v !== value) : [...values, value]);
  return (
    <Menu.Root>
      <Menu.Trigger
        disabled={disabled}
        aria-label={aria["aria-label"]}
        className={cn(
          "flex min-h-7 w-full items-center justify-between gap-2 rounded border border-line-strong bg-surface px-2 py-0.5 text-left text-sm text-ink shadow-xs",
          "hover:border-ink-subtle disabled:cursor-not-allowed disabled:opacity-60",
          className,
        )}
      >
        <span className="flex min-w-0 flex-wrap gap-1">
          {selected.length === 0 ? (
            <span className="text-ink-subtle">{placeholder}</span>
          ) : (
            selected.map((o) => <span key={o.value} className="rounded-sm bg-sunken px-1.5 py-0.5 text-xs text-ink">{o.label}</span>)
          )}
        </span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0 text-ink-subtle" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Content align="start" sideOffset={4} className="z-50 max-h-72 min-w-[12rem] overflow-y-auto rounded-lg border border-line bg-raised p-1 shadow-overlay data-[state=open]:animate-pop-in">
          {options.length === 0 && <div className="px-2 py-1.5 text-sm text-ink-subtle">Nothing to choose from</div>}
          {options.map((option) => (
            <Menu.CheckboxItem
              key={option.value}
              checked={values.includes(option.value)}
              onCheckedChange={() => toggle(option.value)}
              // Keep the menu open so several options can be toggled in one go.
              onSelect={(e) => e.preventDefault()}
              className="relative flex h-8 cursor-default select-none items-center rounded pl-7 pr-2 text-sm text-ink outline-none data-[highlighted]:bg-sunken"
            >
              <Menu.ItemIndicator className="absolute left-2"><Check className="h-3.5 w-3.5 text-accent" /></Menu.ItemIndicator>
              {option.label}
            </Menu.CheckboxItem>
          ))}
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
  );
}
