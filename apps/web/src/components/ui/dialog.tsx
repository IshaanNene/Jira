import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import type { ReactNode } from "react";

import { cn } from "@/utils/cn";

export const Dialog = DialogPrimitive.Root;
export const DialogTrigger = DialogPrimitive.Trigger;
export const DialogClose = DialogPrimitive.Close;

interface DialogContentProps {
  title: string;
  description?: string;
  children: ReactNode;
  className?: string;
}

/** Modal surface. Radix supplies the focus trap, Escape handling and aria wiring. */
export function DialogContent({ title, description, children, className }: DialogContentProps) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-ink/40 backdrop-blur-[1px] data-[state=open]:animate-fade-in" />
      <DialogPrimitive.Content
        className={cn(
          "fixed left-1/2 top-[18%] z-50 w-[calc(100vw-2rem)] max-w-md -translate-x-1/2",
          "rounded-lg border border-line bg-raised p-5 shadow-overlay data-[state=open]:animate-pop-in",
          className,
        )}
      >
        <div className="mb-4 pr-8">
          <DialogPrimitive.Title className="text-lg font-semibold text-ink">{title}</DialogPrimitive.Title>
          {description && <DialogPrimitive.Description className="mt-1 text-sm text-ink-muted">{description}</DialogPrimitive.Description>}
        </div>
        {children}
        <DialogPrimitive.Close
          className="absolute right-3 top-3 flex h-7 w-7 items-center justify-center rounded text-ink-subtle hover:bg-sunken hover:text-ink"
          aria-label="Close"
        >
          <X className="h-4 w-4" />
        </DialogPrimitive.Close>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

export function DialogFooter({ children }: { children: ReactNode }) {
  return <div className="mt-5 flex justify-end gap-2">{children}</div>;
}
