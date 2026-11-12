import * as ToastPrimitive from "@radix-ui/react-toast";
import { AlertTriangle, CheckCircle2, X } from "lucide-react";
import { create } from "zustand";

import { cn } from "@/utils/cn";

interface ToastItem {
  id: number;
  tone: "success" | "error";
  title: string;
  description?: string;
}

interface ToastState {
  items: ToastItem[];
  push: (toast: Omit<ToastItem, "id">) => void;
  dismiss: (id: number) => void;
}

let nextId = 1;

// Transient UI state: belongs in a client store, not in the server cache.
const useToastStore = create<ToastState>((set) => ({
  items: [],
  push: (toast) => set((s) => ({ items: [...s.items, { ...toast, id: nextId++ }] })),
  dismiss: (id) => set((s) => ({ items: s.items.filter((t) => t.id !== id) })),
}));

/** Imperative API so mutations can report outcomes from anywhere. */
export const toast = {
  success: (title: string, description?: string) => useToastStore.getState().push({ tone: "success", title, description }),
  error: (title: string, description?: string) => useToastStore.getState().push({ tone: "error", title, description }),
};

export function Toaster() {
  const { items, dismiss } = useToastStore();
  return (
    <ToastPrimitive.Provider duration={5000} swipeDirection="right">
      {items.map((item) => (
        <ToastPrimitive.Root
          key={item.id}
          onOpenChange={(open) => !open && dismiss(item.id)}
          className={cn(
            "flex items-start gap-3 rounded-lg border border-line bg-raised p-3 shadow-overlay",
            "data-[state=open]:animate-pop-in",
          )}
        >
          {item.tone === "success" ? (
            <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-success" aria-hidden />
          ) : (
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-danger" aria-hidden />
          )}
          <div className="min-w-0 flex-1">
            <ToastPrimitive.Title className="text-sm font-medium text-ink">{item.title}</ToastPrimitive.Title>
            {item.description && <ToastPrimitive.Description className="mt-0.5 text-sm text-ink-muted">{item.description}</ToastPrimitive.Description>}
          </div>
          <ToastPrimitive.Close className="rounded p-0.5 text-ink-subtle hover:text-ink" aria-label="Dismiss">
            <X className="h-3.5 w-3.5" />
          </ToastPrimitive.Close>
        </ToastPrimitive.Root>
      ))}
      <ToastPrimitive.Viewport className="fixed bottom-4 right-4 z-[60] flex w-[22rem] max-w-[calc(100vw-2rem)] flex-col gap-2 outline-none" />
    </ToastPrimitive.Provider>
  );
}
