import Link from "@tiptap/extension-link";
import Placeholder from "@tiptap/extension-placeholder";
import { EditorContent, useEditor, type Editor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { Bold, Code, Italic, List, ListOrdered, Quote, Strikethrough } from "lucide-react";
import { useEffect } from "react";

import type { RichDoc } from "@/services/api";
import { cn } from "@/utils/cn";

// The same node and mark set the server's validator accepts. Links are restricted to
// safe schemes here too; the server check is the one that counts.
const extensions = (placeholder?: string) => [
  StarterKit,
  Link.configure({ openOnClick: false, autolink: true, protocols: ["http", "https", "mailto"], HTMLAttributes: { rel: "noopener noreferrer nofollow", target: "_blank" } }),
  ...(placeholder ? [Placeholder.configure({ placeholder })] : []),
];

export function isEmptyDoc(doc: RichDoc | null | undefined): boolean {
  if (!doc) return true;
  const hasText = (node: unknown): boolean => {
    if (!node || typeof node !== "object") return false;
    const n = node as { text?: string; content?: unknown[] };
    return Boolean(n.text?.trim()) || (n.content ?? []).some(hasText);
  };
  return !hasText(doc);
}

/** Read-only rendering through the editor schema — stored documents are never treated as HTML. */
export function RichTextView({ doc, className }: { doc: RichDoc; className?: string }) {
  const editor = useEditor({ extensions: extensions(), content: doc, editable: false, editorProps: { attributes: { class: cn("rich-text", className) } } });
  useEffect(() => {
    if (editor && JSON.stringify(editor.getJSON()) !== JSON.stringify(doc)) editor.commands.setContent(doc, false);
  }, [editor, doc]);
  return <EditorContent editor={editor} />;
}

function ToolbarButton({ editor, label, active, onClick, icon: Icon }: { editor: Editor; label: string; active: boolean; onClick: () => void; icon: typeof Bold }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={active}
      disabled={!editor.isEditable}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={cn("flex h-7 w-7 items-center justify-center rounded text-ink-muted hover:bg-sunken hover:text-ink", active && "bg-accent-soft text-accent")}
    >
      <Icon className="h-3.5 w-3.5" />
    </button>
  );
}

interface EditorProps {
  initial?: RichDoc | null;
  placeholder?: string;
  autoFocus?: boolean;
  onChange: (doc: RichDoc) => void;
  /** Change this to reset the editor's content (e.g. after posting a comment). */
  resetKey?: number;
  minHeight?: string;
  "aria-label": string;
}

export function RichTextEditor({ initial, placeholder, autoFocus, onChange, resetKey, minHeight = "6rem", ...aria }: EditorProps) {
  const editor = useEditor({
    extensions: extensions(placeholder),
    content: initial ?? undefined,
    autofocus: autoFocus ? "end" : false,
    onUpdate: ({ editor: e }) => onChange(e.getJSON() as RichDoc),
    editorProps: { attributes: { class: "rich-text px-3 py-2", style: `min-height:${minHeight}`, role: "textbox", "aria-multiline": "true", "aria-label": aria["aria-label"] } },
  });
  useEffect(() => {
    if (editor && resetKey !== undefined) editor.commands.clearContent();
  }, [editor, resetKey]);
  if (!editor) return <div className="rounded border border-line-strong" style={{ minHeight }} />;
  const chain = () => editor.chain().focus();
  return (
    <div className="rounded border border-line-strong bg-surface focus-within:border-accent focus-within:ring-2 focus-within:ring-accent/25">
      <div className="flex items-center gap-0.5 border-b border-line px-1.5 py-1" role="toolbar" aria-label="Formatting">
        <ToolbarButton editor={editor} label="Bold" icon={Bold} active={editor.isActive("bold")} onClick={() => chain().toggleBold().run()} />
        <ToolbarButton editor={editor} label="Italic" icon={Italic} active={editor.isActive("italic")} onClick={() => chain().toggleItalic().run()} />
        <ToolbarButton editor={editor} label="Strikethrough" icon={Strikethrough} active={editor.isActive("strike")} onClick={() => chain().toggleStrike().run()} />
        <ToolbarButton editor={editor} label="Code" icon={Code} active={editor.isActive("code")} onClick={() => chain().toggleCode().run()} />
        <span className="mx-1 h-4 w-px bg-line" />
        <ToolbarButton editor={editor} label="Bulleted list" icon={List} active={editor.isActive("bulletList")} onClick={() => chain().toggleBulletList().run()} />
        <ToolbarButton editor={editor} label="Numbered list" icon={ListOrdered} active={editor.isActive("orderedList")} onClick={() => chain().toggleOrderedList().run()} />
        <ToolbarButton editor={editor} label="Quote" icon={Quote} active={editor.isActive("blockquote")} onClick={() => chain().toggleBlockquote().run()} />
      </div>
      <EditorContent editor={editor} />
    </div>
  );
}
