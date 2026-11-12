import { Download, FileText, ImageIcon, Paperclip, Trash2 } from "lucide-react";
import { useRef, useState, type DragEvent } from "react";

import { Button, ErrorState, Skeleton, toast } from "@/components/ui";
import { useUserDirectory } from "@/modules/organizations";
import { download, errorMessage, type Attachment, type IssueDetail } from "@/services/api";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";
import { formatRelative } from "@/utils/format";

import { attachmentContentPath, MAX_ATTACHMENT_BYTES, useAttachmentMutations, useAttachments } from "../api";

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function Attachments({ issue, canEdit, canModerate }: { issue: IssueDetail; canEdit: boolean; canModerate: boolean }) {
  const me = useSession((s) => s.account?.id);
  const { byId } = useUserDirectory();
  const attachments = useAttachments(issue.key);
  const { upload, remove } = useAttachmentMutations(issue.key);
  const input = useRef<HTMLInputElement>(null);
  const [dragging, setDragging] = useState(false);
  const rows = attachments.data ?? [];

  const add = (files: FileList | File[] | null) => {
    for (const file of Array.from(files ?? [])) {
      // Checked here for an instant answer; the server enforces the same limit.
      if (file.size > MAX_ATTACHMENT_BYTES) {
        toast.error(`${file.name} is too large`, "Attachments can be up to 10 MB.");
        continue;
      }
      upload.mutate(file, { onError: (error) => toast.error(`Couldn't attach ${file.name}`, errorMessage(error)) });
    }
    if (input.current) input.current.value = "";
  };

  const save = async (attachment: Attachment) => {
    try {
      // Fetched with the session's token, then handed to the browser as a download.
      const blob = await download(attachmentContentPath(issue.key, attachment.id));
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = attachment.filename;
      link.click();
      URL.revokeObjectURL(url);
    } catch (error) {
      toast.error("Couldn't download the file", errorMessage(error));
    }
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragging(false);
    if (canEdit) add(e.dataTransfer.files);
  };

  if (!canEdit && rows.length === 0) return null;
  return (
    <section
      onDragOver={(e) => { if (canEdit) { e.preventDefault(); setDragging(true); } }}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
      className={cn("rounded", dragging && "outline-dashed outline-2 outline-offset-4 outline-accent")}
    >
      <div className="mb-2 flex items-center justify-between">
        <h2 className="text-base font-semibold text-ink">Attachments{rows.length > 0 && <span className="ml-1.5 text-sm font-normal text-ink-subtle">{rows.length}</span>}</h2>
        {canEdit && (
          <>
            <input ref={input} type="file" multiple className="sr-only" aria-label="Attach files" onChange={(e) => add(e.target.files)} />
            <Button size="sm" variant="ghost" loading={upload.isPending} onClick={() => input.current?.click()}>
              <Paperclip className="h-3.5 w-3.5" aria-hidden /> Attach
            </Button>
          </>
        )}
      </div>
      {attachments.isPending ? (
        <Skeleton className="h-9" />
      ) : attachments.isError ? (
        <ErrorState error={attachments.error} onRetry={() => void attachments.refetch()} />
      ) : rows.length === 0 ? (
        <p className="text-sm text-ink-subtle">No attachments. Drop files here or use Attach (up to 10 MB each).</p>
      ) : (
        <ul className="divide-y divide-line rounded border border-line">
          {rows.map((a) => {
            const Icon = a.content_type.startsWith("image/") ? ImageIcon : FileText;
            return (
              <li key={a.id} className="flex items-center gap-2.5 px-2.5 py-1.5 hover:bg-sunken">
                <Icon className="h-4 w-4 shrink-0 text-ink-subtle" aria-hidden />
                <button type="button" onClick={() => void save(a)} className="min-w-0 flex-1 truncate text-left text-sm font-medium text-accent hover:underline" title={`Download ${a.filename}`}>
                  {a.filename}
                </button>
                <span className="hidden shrink-0 text-xs text-ink-subtle sm:inline">
                  {formatBytes(a.size_bytes)} · {byId.get(a.uploader_id)?.display_name ?? "Former member"} · {formatRelative(a.created_at)}
                </span>
                <button type="button" aria-label={`Download ${a.filename}`} onClick={() => void save(a)} className="rounded p-1 text-ink-subtle hover:bg-line hover:text-ink">
                  <Download className="h-3.5 w-3.5" />
                </button>
                {canEdit && (a.uploader_id === me || canModerate) && (
                  <button
                    type="button"
                    aria-label={`Delete ${a.filename}`}
                    className="rounded p-1 text-ink-subtle hover:bg-line hover:text-danger"
                    onClick={() => window.confirm(`Delete ${a.filename}?`) && remove.mutate(a.id, { onError: (error) => toast.error("Couldn't delete the file", errorMessage(error)) })}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
