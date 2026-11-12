import { toast } from "@/components/ui";
import { ApiError, errorMessage } from "@/services/api";

import { useUpdateIssue, type IssuePatch } from "../api";

/**
 * Inline-edit helper for the issue view. Failures are always explained: a conflict
 * means someone else saved first, and the view now shows their version.
 */
export function useIssueEdit(issueKey: string) {
  const update = useUpdateIssue(issueKey);
  const save = (patch: IssuePatch, onDone?: () => void) =>
    update.mutate(patch, {
      onSuccess: onDone,
      onError: (error) => {
        if (error instanceof ApiError && error.isConflict) {
          toast.error("Someone else updated this issue", "Their changes are now shown. Check them, then make your change again.");
        } else {
          // Field-level reasons (for example a workflow rule) are more useful than the generic summary.
          const reason = error instanceof ApiError && error.fieldErrors[0] ? error.fieldErrors[0].message : errorMessage(error);
          toast.error("Couldn't save your change", reason);
        }
      },
    });
  return { save, saving: update.isPending };
}
