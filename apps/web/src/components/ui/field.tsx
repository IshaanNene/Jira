import * as LabelPrimitive from "@radix-ui/react-label";
import { cloneElement, isValidElement, useId, type ReactElement, type ReactNode } from "react";

import { cn } from "@/utils/cn";

interface FieldProps {
  label: string;
  /** Validation message. When present the control is marked invalid and described by it. */
  error?: string;
  hint?: ReactNode;
  className?: string;
  /** A single form control; `id`, `aria-describedby` and `invalid` are wired automatically. */
  children: ReactElement<{ id?: string; "aria-describedby"?: string; invalid?: boolean }>;
}

/** Label + control + hint/error, with the accessibility wiring done once. */
export function Field({ label, error, hint, className, children }: FieldProps) {
  const id = useId();
  const messageId = `${id}-message`;
  const control = isValidElement(children)
    ? cloneElement(children, {
        id: children.props.id ?? id,
        "aria-describedby": error || hint ? messageId : undefined,
        invalid: Boolean(error),
      })
    : children;
  return (
    <div className={cn("space-y-1.5", className)}>
      <LabelPrimitive.Root htmlFor={children.props.id ?? id} className="block text-sm font-medium text-ink">
        {label}
      </LabelPrimitive.Root>
      {control}
      {error ? (
        <p id={messageId} role="alert" className="text-sm text-danger">
          {error}
        </p>
      ) : hint ? (
        <p id={messageId} className="text-sm text-ink-subtle">
          {hint}
        </p>
      ) : null}
    </div>
  );
}
