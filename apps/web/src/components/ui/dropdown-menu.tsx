import * as Menu from "@radix-ui/react-dropdown-menu";
import { Check } from "lucide-react";
import { forwardRef, type ComponentPropsWithoutRef } from "react";

import { cn } from "@/utils/cn";

export const DropdownMenu = Menu.Root;
export const DropdownMenuTrigger = Menu.Trigger;
export const DropdownMenuGroup = Menu.Group;

export const DropdownMenuContent = forwardRef<HTMLDivElement, ComponentPropsWithoutRef<typeof Menu.Content>>(
  function DropdownMenuContent({ className, sideOffset = 6, ...props }, ref) {
    return (
      <Menu.Portal>
        <Menu.Content
          ref={ref}
          sideOffset={sideOffset}
          className={cn(
            "z-50 min-w-[12rem] overflow-hidden rounded-lg border border-line bg-raised p-1 shadow-overlay",
            "data-[state=open]:animate-pop-in",
            className,
          )}
          {...props}
        />
      </Menu.Portal>
    );
  },
);

interface ItemProps extends ComponentPropsWithoutRef<typeof Menu.Item> {
  tone?: "default" | "danger";
  selected?: boolean;
}

export const DropdownMenuItem = forwardRef<HTMLDivElement, ItemProps>(function DropdownMenuItem(
  { className, tone = "default", selected, children, ...props },
  ref,
) {
  return (
    <Menu.Item
      ref={ref}
      className={cn(
        "flex h-8 cursor-default select-none items-center gap-2 rounded px-2 text-sm outline-none",
        "data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
        tone === "danger" ? "text-danger data-[highlighted]:bg-danger-soft" : "text-ink data-[highlighted]:bg-sunken",
        className,
      )}
      {...props}
    >
      {children}
      {selected && <Check className="ml-auto h-3.5 w-3.5 text-accent" aria-label="Selected" />}
    </Menu.Item>
  );
});

export function DropdownMenuLabel({ className, ...props }: ComponentPropsWithoutRef<typeof Menu.Label>) {
  return <Menu.Label className={cn("px-2 py-1.5 text-2xs font-semibold uppercase tracking-wide text-ink-subtle", className)} {...props} />;
}

export function DropdownMenuSeparator() {
  return <Menu.Separator className="-mx-1 my-1 h-px bg-line" />;
}
