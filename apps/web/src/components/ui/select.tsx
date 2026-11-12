import * as SelectPrimitive from "@radix-ui/react-select";
import { Check, ChevronDown } from "lucide-react";

import { cn } from "@/utils/cn";

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
}

interface SelectProps {
  value: string;
  onValueChange: (value: string) => void;
  options: SelectOption[];
  id?: string;
  disabled?: boolean;
  invalid?: boolean;
  "aria-label"?: string;
  "aria-describedby"?: string;
  className?: string;
  size?: "sm" | "md";
}

/** Single-choice picker with full keyboard support (Radix Select). */
export function Select({ value, onValueChange, options, id, disabled, invalid, className, size = "md", ...aria }: SelectProps) {
  return (
    <SelectPrimitive.Root value={value} onValueChange={onValueChange} disabled={disabled}>
      <SelectPrimitive.Trigger
        id={id}
        aria-invalid={invalid || undefined}
        className={cn(
          "inline-flex w-full items-center justify-between gap-2 rounded border bg-surface text-ink shadow-xs",
          "transition-colors duration-100 hover:border-ink-subtle disabled:cursor-not-allowed disabled:opacity-60",
          size === "sm" ? "h-7 px-2 text-sm" : "h-9 px-3 text-base",
          invalid ? "border-danger" : "border-line-strong",
          className,
        )}
        {...aria}
      >
        <SelectPrimitive.Value />
        <SelectPrimitive.Icon>
          <ChevronDown className="h-3.5 w-3.5 text-ink-subtle" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal>
        <SelectPrimitive.Content
          position="popper"
          sideOffset={4}
          className="z-50 min-w-[var(--radix-select-trigger-width)] overflow-hidden rounded-lg border border-line bg-raised p-1 shadow-overlay data-[state=open]:animate-pop-in"
        >
          <SelectPrimitive.Viewport>
            {options.map((option) => (
              <SelectPrimitive.Item
                key={option.value}
                value={option.value}
                className="relative flex cursor-default select-none flex-col rounded py-1.5 pl-2 pr-8 text-sm text-ink outline-none data-[highlighted]:bg-sunken"
              >
                <SelectPrimitive.ItemText>{option.label}</SelectPrimitive.ItemText>
                {option.description && <span className="text-xs text-ink-subtle">{option.description}</span>}
                <SelectPrimitive.ItemIndicator className="absolute right-2 top-2">
                  <Check className="h-3.5 w-3.5 text-accent" />
                </SelectPrimitive.ItemIndicator>
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
}
