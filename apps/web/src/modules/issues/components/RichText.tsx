import Link from "@tiptap/extension-link";
import Mention from "@tiptap/extension-mention";
import Placeholder from "@tiptap/extension-placeholder";
import { EditorContent, useEditor, type Editor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import type { SuggestionKeyDownProps, SuggestionOptions, SuggestionProps } from "@tiptap/suggestion";
import { Bold, Code, Italic, List, ListOrdered, Quote, Strikethrough } from "lucide-react";
import { useEffect, useId, useReducer, useRef } from "react";
import { createPortal } from "react-dom";

import { can } from "@/modules/organizations";
import { api, type Member, type MemberPage, type RichDoc } from "@/services/api";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";

type MentionSuggestion = Omit<SuggestionOptions<Member>, "editor">;

// The same node and mark set the server's validator accepts. Links are restricted to
// safe schemes here too; the server check is the one that counts. A mention stores the
// person's id and the name shown when it was written; the server notifies by the id.
const extensions = (placeholder?: string, suggestion?: MentionSuggestion) => [
  StarterKit,
  Link.configure({ openOnClick: false, autolink: true, protocols: ["http", "https", "mailto"], HTMLAttributes: { rel: "noopener noreferrer nofollow", target: "_blank" } }),
  Mention.configure({ HTMLAttributes: { class: "mention" }, ...(suggestion ? { suggestion } : {}) }),
  ...(placeholder ? [Placeholder.configure({ placeholder })] : []),
];

/** Active members matching what follows the @. Guests cannot list people, so they get none. */
async function findPeople(query: string): Promise<Member[]> {
  if (!can(useSession.getState().organization?.role, "viewMembers")) return [];
  try {
    const page = await api.get<MemberPage>("/api/v1/organizations/current/members", { query: { limit: 8, query: query.trim() || undefined } });
    return page.items.filter((m) => m.status === "active");
  } catch {
    return [];
  }
}

interface MentionMenu {
  items: Member[];
  index: number;
  rect: DOMRect | null;
  pick: (member: Member) => void;
}

/**
 * The @ picker. Tiptap drives it through callbacks outside React, so the open menu lives in a
 * ref (the callbacks always see the latest) and a counter re-renders the popup.
 */
function useMentionPicker() {
  const menu = useRef<MentionMenu | null>(null);
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const set = (next: MentionMenu | null) => {
    menu.current = next;
    rerender();
  };
  const suggestion = useRef<MentionSuggestion>({
    char: "@",
    items: ({ query }) => findPeople(query),
    render: () => {
      const show = (props: SuggestionProps<Member>) =>
        set({
          items: props.items,
          index: 0,
          rect: props.clientRect?.() ?? null,
          pick: (m) => props.command({ id: m.user_id, label: m.display_name }),
        });
      return {
        onStart: show,
        onUpdate: show,
        onKeyDown: ({ event }: SuggestionKeyDownProps) => {
          const open = menu.current;
          if (!open) return false;
          if (event.key === "Escape") {
            set(null);
            return true;
          }
          if (open.items.length === 0) return false;
          const n = open.items.length;
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            set({ ...open, index: (open.index + (event.key === "ArrowDown" ? 1 : n - 1)) % n });
            return true;
          }
          const chosen = open.items[open.index];
          if (chosen && (event.key === "Enter" || event.key === "Tab")) {
            open.pick(chosen);
            return true;
          }
          return false;
        },
        onExit: () => set(null),
      };
    },
  }).current;
  return { suggestion, menu: menu.current };
}

function MentionList({ menu, listId }: { menu: MentionMenu; listId: string }) {
  if (menu.items.length === 0 || !menu.rect) return null;
  return createPortal(
    <ul
      id={listId}
      role="listbox"
      aria-label="People"
      style={{ position: "fixed", left: menu.rect.left, top: menu.rect.bottom + 4 }}
      className="z-50 max-h-72 w-64 overflow-auto rounded border border-line bg-surface py-1 shadow-overlay"
    >
      {menu.items.map((m, i) => (
        <li
          key={m.user_id}
          role="option"
          aria-selected={i === menu.index}
          // Keep focus in the editor while choosing.
          onMouseDown={(e) => {
            e.preventDefault();
            menu.pick(m);
          }}
          className={cn("cursor-pointer px-3 py-1.5 text-sm", i === menu.index ? "bg-accent-soft text-ink" : "text-ink-muted hover:bg-sunken")}
        >
          <div className="truncate font-medium text-ink">{m.display_name}</div>
          <div className="truncate text-xs text-ink-subtle">{m.email}</div>
        </li>
      ))}
    </ul>,
    document.body,
  );
}

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
  const { suggestion, menu } = useMentionPicker();
  const listId = useId();
  const editor = useEditor({
    extensions: extensions(placeholder, suggestion),
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
      {menu && <MentionList menu={menu} listId={listId} />}
    </div>
  );
}
