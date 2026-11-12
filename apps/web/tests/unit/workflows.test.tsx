import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { addStatus, addTransition, describeFrom, edgePoints, moveStatus, removeStatus, slugify, updateTransition } from "@/modules/workflows/model";
import WorkflowDesignerPage from "@/modules/workflows/pages/WorkflowDesignerPage";
import type { WorkflowDefinition } from "@/services/api";

import { fakeApi, META, renderAt, signIn } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const definition = (): WorkflowDefinition => ({
  statuses: [
    { key: "todo", name: "To Do", category: "todo", x: 60, y: 120 },
    { key: "doing", name: "Doing", category: "in_progress", x: 300, y: 120 },
    { key: "done", name: "Done", category: "done", x: 540, y: 120 },
  ],
  initial: "todo",
  transitions: [
    { id: "start", name: "Start", from: ["todo"], to: "doing", conditions: [], validators: [], post_functions: [] },
    { id: "finish", name: "Finish", from: ["doing"], to: "done", conditions: [], validators: [], post_functions: [] },
    { id: "reopen", name: "Reopen", from: [], to: "todo", conditions: [], validators: [], post_functions: [] },
  ],
});

describe("workflow editing model", () => {
  it("adds statuses with unique keys and without overlapping existing ones", () => {
    const first = addStatus(definition(), "Code Review");
    expect(first.key).toBe("code_review");
    const second = addStatus(first.definition, "Code review");
    expect(second.key).toBe("code_review_2");
    const added = second.definition.statuses.at(-1)!;
    expect(added.y).toBeGreaterThan(Math.max(...definition().statuses.map((s) => s.y ?? 0)));
    expect(slugify("  QA / Sign-off! ")).toBe("qa_sign_off");
    expect(slugify("2nd pass")).toMatch(/^[a-z]/);
  });

  it("the first status of an empty workflow becomes the initial one", () => {
    const { definition: d, key } = addStatus({ statuses: [], initial: "", transitions: [] }, "Open");
    expect(d.initial).toBe(key);
  });

  it("removing a status removes what depended on it but never widens a transition", () => {
    const d = removeStatus(definition(), "doing");
    expect(d.statuses.map((s) => s.key)).toEqual(["todo", "done"]);
    // start led to it, finish started only from it: both go. The global reopen stays.
    expect(d.transitions.map((t) => t.id)).toEqual(["reopen"]);

    const multi = updateTransition(definition(), "finish", { from: ["doing", "todo"] });
    const kept = removeStatus(multi, "doing").transitions.find((t) => t.id === "finish");
    expect(kept?.from).toEqual(["todo"]);

    expect(removeStatus(definition(), "todo").initial).toBe("doing");
  });

  it("connects, moves on a grid and describes transitions", () => {
    const connected = addTransition(definition(), "done", "doing");
    const t = connected.definition.transitions.find((x) => x.id === connected.id)!;
    expect([t.from, t.to, t.name]).toEqual([["done"], "doing", "Doing"]);
    expect(moveStatus(definition(), "todo", 33, -500).statuses[0]).toMatchObject({ x: 100, y: 0 });
    expect(describeFrom(definition(), definition().transitions[0]!)).toBe("To Do");
    expect(describeFrom(definition(), definition().transitions[2]!)).toBe("Any status");
  });

  it("draws opposite directions as separate lines", () => {
    const [a, b] = definition().statuses as [WorkflowDefinition["statuses"][number], WorkflowDefinition["statuses"][number]];
    const forward = edgePoints(a, b);
    const back = edgePoints(b, a);
    expect(forward.start.y).not.toBe(back.end.y);
    expect(forward.start.x).toBeLessThan(forward.end.x);
  });
});

function detail(overrides: Record<string, unknown> = {}) {
  return {
    id: "w1", name: "Delivery", description: "", published_version: 1, has_draft: false, version: 5, updated_at: "2026-11-01T00:00:00Z",
    draft: null, published: definition(), problems: [], assignment_count: 1,
    ...overrides,
  };
}
const routes = (extra: Record<string, unknown> = {}) => ({
  "GET /api/v1/workflows/w1": detail(),
  "GET /api/v1/custom-fields": { items: [] },
  "GET /api/v1/issues/meta": META,
  ...extra,
});
const renderDesigner = () => renderAt(<WorkflowDesignerPage />, "/settings/workflows/:workflowId", "/settings/workflows/w1");

describe("WorkflowDesignerPage", () => {
  it("shows the published workflow and lets an admin add a status and save a draft", async () => {
    signIn("admin");
    const api = fakeApi(
      routes({
        "PUT /api/v1/workflows/w1/draft": (call: { body: unknown }) => ({
          body: detail({ draft: (call.body as { definition: WorkflowDefinition }).definition, has_draft: true, version: 6, problems: [{ path: "statuses[3]", message: "No transition leads to “QA”, so issues can never reach it." }] }),
        }),
      }),
    );
    renderDesigner();
    expect(await screen.findByRole("button", { name: "To Do, initial status" })).toBeInTheDocument();
    expect(screen.getByText("Published v1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save draft" })).toBeDisabled();

    await userEvent.type(screen.getByLabelText("New status name"), "QA");
    await userEvent.click(screen.getByRole("button", { name: /Add status/ }));
    expect(screen.getByRole("button", { name: "QA" })).toBeInTheDocument();
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Save draft" }));
    await waitFor(() => expect(api.sent("PUT", "/api/v1/workflows/w1/draft")).toHaveLength(1));
    const call = api.sent("PUT", "/api/v1/workflows/w1/draft")[0]!;
    expect(call.headers["If-Match"]).toBe('"5"');
    expect((call.body as { definition: WorkflowDefinition }).definition.statuses.map((s) => s.key)).toEqual(["todo", "doing", "done", "qa"]);
    expect(await screen.findByText("Unpublished draft")).toBeInTheDocument();
  });

  it("does not publish a workflow that has problems, and says why", async () => {
    signIn("admin");
    const problem = "No transition leads to “QA”, so issues can never reach it.";
    const api = fakeApi(routes({ "GET /api/v1/workflows/w1": detail({ has_draft: true, draft: definition(), problems: [{ path: "statuses[3]", message: problem }] }) }));
    renderDesigner();
    expect(await screen.findByText(problem)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Publish" }));
    expect(await screen.findByText("This workflow can't be published yet")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/workflows/w1/publish")).toHaveLength(0);
  });

  it("publishes a clean draft with the version it was based on", async () => {
    signIn("admin");
    const api = fakeApi(
      routes({
        "GET /api/v1/workflows/w1": detail({ has_draft: true, draft: definition(), version: 7 }),
        "POST /api/v1/workflows/w1/publish": () => ({ body: detail({ published_version: 2, version: 8 }) }),
      }),
    );
    renderDesigner();
    await userEvent.click(await screen.findByRole("button", { name: "Publish" }));
    expect(await screen.findByText("Published version 2")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/workflows/w1/publish")[0]!.headers["If-Match"]).toBe('"7"');
    expect(await screen.findByText("Published v2")).toBeInTheDocument();
  });

  it("edits a transition's rules from the side panel", async () => {
    signIn("admin");
    fakeApi(routes());
    renderDesigner();
    await userEvent.click(await screen.findByRole("button", { name: "Finish" }));
    const name = screen.getByLabelText("Name");
    expect(name).toHaveValue("Finish");
    await userEvent.clear(name);
    await userEvent.type(name, "Ship it");
    expect(screen.getByRole("button", { name: "Ship it" })).toBeInTheDocument();
    expect(screen.getByText("Anyone who can edit issues in the project.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Delete transition/ }));
    expect(screen.queryByRole("button", { name: "Ship it" })).not.toBeInTheDocument();
    expect(screen.getByText("Transitions (2)")).toBeInTheDocument();
  });

  it("is view-only for people who are not workspace admins", async () => {
    signIn("member");
    fakeApi(routes());
    renderDesigner();
    expect(await screen.findByText(/View only/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Publish" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("New status name")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Doing" }));
    expect(screen.getByLabelText("Name")).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Delete status/ })).not.toBeInTheDocument();
  });
});
