import type { Editor } from "@tiptap/react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RichTextEditor, RichTextView } from "@/modules/issues/components/RichText";
import type { RichDoc } from "@/services/api";

import { fakeApi, MEMBERS, signIn } from "./harness";

afterEach(() => vi.unstubAllGlobals());

// ProseMirror's own DOM input handling is not available in jsdom, so text goes in through the
// editor that Tiptap attaches to its element; keys go through ProseMirror's key handling.
function editorOf(textbox: HTMLElement): Editor {
  return (textbox as HTMLElement & { editor: Editor }).editor;
}

describe("@mentions", () => {
  it("lists matching people and inserts the chosen one as a mention of their id", async () => {
    signIn("member");
    const api = fakeApi({ "GET /api/v1/organizations/current/members": MEMBERS });
    const onChange = vi.fn<(doc: RichDoc) => void>();
    render(<RichTextEditor aria-label="Comment" onChange={onChange} />);
    const textbox = await screen.findByRole("textbox", { name: "Comment" });
    await act(async () => {
      editorOf(textbox).chain().focus().insertContent("Thanks @ad").run();
      await new Promise((r) => setTimeout(r, 10));
    });

    const option = await screen.findByRole("option", { name: /Ada Lovelace/ });
    expect(option).toHaveAttribute("aria-selected", "true");
    const lookup = api.calls.find((c) => c.path === "/api/v1/organizations/current/members");
    expect(lookup?.query.get("query")).toBe("ad");

    act(() => void fireEvent.keyDown(textbox, { key: "Enter" }));
    await waitFor(() => expect(screen.queryByRole("listbox", { name: "People" })).not.toBeInTheDocument());
    const doc = onChange.mock.calls.at(-1)![0] as { content: { content: { type: string; attrs?: Record<string, unknown> }[] }[] };
    const mention = doc.content[0]?.content.find((n) => n.type === "mention");
    expect(mention?.attrs).toMatchObject({ id: MEMBERS.items[0]?.user_id, label: "Ada Lovelace" });
  });

  it("offers guests no one, since they cannot list people", async () => {
    signIn("guest");
    const api = fakeApi({ "GET /api/v1/organizations/current/members": MEMBERS });
    render(<RichTextEditor aria-label="Comment" onChange={() => {}} />);
    const textbox = await screen.findByRole("textbox", { name: "Comment" });
    await act(async () => {
      editorOf(textbox).chain().focus().insertContent("@ad").run();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(screen.queryByRole("listbox", { name: "People" })).not.toBeInTheDocument();
    expect(api.calls.some((c) => c.path === "/api/v1/organizations/current/members")).toBe(false);
  });

  it("shows stored mentions as @name", async () => {
    const doc = {
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "Ask " }, { type: "mention", attrs: { id: MEMBERS.items[0]?.user_id, label: "Ada Lovelace" } }] }],
    } as RichDoc;
    render(<RichTextView doc={doc} />);
    const chip = await screen.findByText("@Ada Lovelace");
    expect(chip).toHaveClass("mention");
  });
});
