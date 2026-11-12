import { useState, type KeyboardEvent } from "react";

import { Input } from "@/components/ui";
import { cn } from "@/utils/cn";

interface InlineInputProps {
  value: string;
  onSave: (value: string) => void;
  type?: string;
  placeholder?: string;
  label: string;
  disabled?: boolean;
  /** Bordered like a normal form control (dialogs) instead of blending into the page. */
  boxed?: boolean;
}

/** Text field that saves on Enter or blur and reverts on Escape. */
export function InlineInput({ value, onSave, type = "text", placeholder, label, disabled, boxed }: InlineInputProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft !== null && draft !== value) onSave(draft);
    setDraft(null);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      e.currentTarget.blur();
    }
    if (e.key === "Escape") {
      setDraft(null);
      const el = e.currentTarget;
      // Blur after state has reset so the revert is what gets committed (nothing).
      requestAnimationFrame(() => el.blur());
    }
  };
  return (
    <Input
      aria-label={label}
      type={type}
      disabled={disabled}
      placeholder={placeholder}
      value={draft ?? value}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={onKeyDown}
      className={cn("h-8 px-2", !boxed && "border-transparent bg-transparent shadow-none hover:border-line-strong hover:bg-sunken focus-visible:bg-surface disabled:hover:border-transparent")}
    />
  );
}
