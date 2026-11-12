import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import ProjectsPage from "@/modules/projects/pages/ProjectsPage";
import { roleAtLeast, suggestKey } from "@/modules/projects/roles";

import { fakeApi, MEMBERS, project, renderAt, signIn } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const routes = (items: unknown[]) => ({
  "GET /api/v1/projects": { items },
  "GET /api/v1/organizations/current/members": MEMBERS,
});

describe("ProjectsPage", () => {
  it("lists projects, marks private ones and filters by name or key", async () => {
    signIn("member");
    fakeApi(routes([project(), project({ id: "p2", key: "OPS", name: "Operations", default_role: null })]));
    renderAt(<ProjectsPage />, "/projects", "/projects");

    expect(await screen.findByRole("link", { name: /Payments/ })).toHaveAttribute("href", "/projects/PAY/issues");
    expect(screen.getByLabelText("Private")).toBeInTheDocument();

    await userEvent.type(screen.getByLabelText("Search projects"), "ops");
    expect(screen.queryByRole("link", { name: /Payments/ })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Operations/ })).toBeInTheDocument();

    await userEvent.clear(screen.getByLabelText("Search projects"));
    await userEvent.type(screen.getByLabelText("Search projects"), "zzz");
    expect(await screen.findByText("No projects match")).toBeInTheDocument();
  });

  it("asks the API for archived projects when that view is chosen", async () => {
    signIn("member");
    const api = fakeApi(routes([]));
    renderAt(<ProjectsPage />, "/projects", "/projects");
    expect(await screen.findByText("No projects yet")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "archived" }));
    await waitFor(() => expect(api.sent("GET", "/api/v1/projects").some((c) => c.query.get("status") === "archived")).toBe(true));
    expect(await screen.findByText("No archived projects")).toBeInTheDocument();
  });

  it("does not offer project creation to guests", async () => {
    signIn("guest");
    fakeApi(routes([]));
    renderAt(<ProjectsPage />, "/projects", "/projects");
    expect(await screen.findByText(/once someone adds you/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create project" })).not.toBeInTheDocument();
  });

  it("shows a retryable error when the list cannot be loaded", async () => {
    signIn("member");
    fakeApi({ "GET /api/v1/projects": () => ({ status: 503, body: { code: "dependency_unavailable", detail: "A required dependency is unavailable. Retry shortly." } }) });
    renderAt(<ProjectsPage />, "/projects", "/projects");
    expect(await screen.findByRole("alert")).toHaveTextContent("A required dependency is unavailable");
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });
});

describe("project helpers", () => {
  it("suggests keys from names", () => {
    expect(suggestKey("Payments Platform")).toBe("PP");
    expect(suggestKey("payments")).toBe("PAYM");
    expect(suggestKey("2 Fast")).toBe(""); // a one-letter key is not valid, so nothing is suggested
    expect(suggestKey("3D Printing Lab")).toBe("PL");
    expect(suggestKey("")).toBe("");
  });
  it("orders project roles", () => {
    expect(roleAtLeast("administrator", "member")).toBe(true);
    expect(roleAtLeast("viewer", "member")).toBe(false);
    expect(roleAtLeast(undefined, "viewer")).toBe(false);
    expect(roleAtLeast("root", "viewer")).toBe(false);
  });
});
