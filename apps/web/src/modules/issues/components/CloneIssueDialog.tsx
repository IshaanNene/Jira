import { useState } from "react";
import { useNavigate } from "react-router-dom";

import { Button, Dialog, DialogContent, DialogFooter, Field, Input, toast } from "@/components/ui";
import { ApiError, errorMessage, type IssueDetail } from "@/services/api";

import { useCloneIssue } from "../api";

/** Copies the issue in its project and opens the copy. */
export function CloneIssueDialog({ issue, onClose }: { issue: IssueDetail; onClose: () => void }) {
  const navigate = useNavigate();
  const clone = useCloneIssue(issue.key, issue.project_key);
  const [summary, setSummary] = useState(`CLONE - ${issue.summary}`.slice(0, 255));
  const trimmed = summary.trim();
  const fieldError = clone.error instanceof ApiError ? clone.error.fieldErrors.find((e) => e.field === "summary")?.message : undefined;

  const submit = () =>
    clone.mutate(trimmed, {
      onSuccess: (copy) => {
        toast.success(`${copy.key} created`, `A copy of ${issue.key}, linked to it.`);
        onClose();
        navigate(`/projects/${copy.project_key}/issues/${copy.key}`);
      },
    });

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        title={`Clone ${issue.key}`}
        description="The copy keeps the type, priority, description, assignee, labels, components, versions, custom fields and dates, starts in the first status and is linked to this issue. Comments, attachments and other links stay here."
      >
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (trimmed) submit();
          }}
        >
          <Field label="Summary" error={!trimmed ? "Enter a summary." : fieldError}>
            <Input value={summary} maxLength={255} autoFocus onChange={(e) => setSummary(e.target.value)} />
          </Field>
          {clone.isError && !fieldError && <p role="alert" className="text-sm text-danger">{errorMessage(clone.error)}</p>}
          <DialogFooter>
            <Button type="button" onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" loading={clone.isPending} disabled={!trimmed}>Clone</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
