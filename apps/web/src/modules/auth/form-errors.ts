import type { FieldValues, Path, UseFormSetError } from "react-hook-form";

import { ApiError, errorMessage } from "@/services/api";

/**
 * Put server-side validation errors next to the fields they belong to. Returns the
 * message to show at form level, or null when every error was attached to a field.
 */
export function applyServerErrors<T extends FieldValues>(error: unknown, setError: UseFormSetError<T>, fields: readonly Path<T>[]): string | null {
  if (error instanceof ApiError && error.fieldErrors.length > 0) {
    const unmatched: string[] = [];
    for (const fieldError of error.fieldErrors) {
      const field = fields.find((f) => f === fieldError.field);
      if (field) setError(field, { type: "server", message: fieldError.message });
      else unmatched.push(fieldError.message);
    }
    // Errors for inputs the form does not register (e.g. custom fields) are shown at form level.
    return unmatched.length > 0 ? unmatched.join(" ") : null;
  }
  return errorMessage(error);
}
