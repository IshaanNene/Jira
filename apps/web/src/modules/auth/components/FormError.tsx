import { AlertTriangle } from "lucide-react";

/** Form-level failure (as opposed to a single field's validation message). */
export function FormError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <div role="alert" className="flex items-start gap-2 rounded border border-danger/30 bg-danger-soft px-3 py-2 text-sm text-danger">
      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden />
      <span>{message}</span>
    </div>
  );
}
