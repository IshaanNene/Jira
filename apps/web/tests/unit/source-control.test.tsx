import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import SourceControlPage from "@/modules/integrations/pages/SourceControlPage";
import type { SourceConnection } from "@/services/api";

import { fakeApi, project, renderAt, signIn, USER_ID } from "./harness";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const connection = (overrides: Partial<SourceConnection> = {}): SourceConnection => ({
  id: "c1",
  provider: "github",
  name: "acme/pay",
  project_keys: ["PAY"],
  payload_path: "/hooks/github/c1",
  last_event_at: null,
  created_by: USER_ID,
  created_at: "2026-01-01T00:00:00Z",
  ...overrides,
});

const routes = (items: SourceConnection[], extra: Record<string, unknown> = {}) => ({
  "GET /api/v1/source-connections": { items },
  "GET /api/v1/projects": { items: [project(), project({ id: "p2", key: "WEB", name: "Website" })] },
  ...extra,
});

const show = () => renderAt(<SourceControlPage />, "/settings/source-control", "/settings/source-control");

describe("source control", () => {
  it("is for admins only", async () => {
    signIn("member");
    fakeApi(routes([]));
    show();
    expect(await screen.findByText("Admins only")).toBeInTheDocument();
  });

  it("connects GitHub for chosen projects and shows the payload URL and secret once", async () => {
    signIn("admin");
    const api = fakeApi(routes([], {
      "POST /api/v1/source-connections": () => ({ status: 201, body: { connection: connection(), secret: "ghsec_0123" } }),
    }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: /connect github/i }));
    const dialog = await screen.findByRole("dialog", { name: "Connect GitHub" });
    await userEvent.type(within(dialog).getByLabelText(/Name/), "acme/pay");
    const connect = within(dialog).getByRole("button", { name: "Connect" });
    expect(connect).toBeDisabled();
    await userEvent.click(await within(dialog).findByRole("checkbox", { name: /PAY/ }));
    await userEvent.click(connect);
    const setup = await screen.findByRole("dialog", { name: "Finish in GitHub" });
    expect(within(setup).getByRole("textbox", { name: "Payload URL" })).toHaveValue(`${window.location.origin}/hooks/github/c1`);
    expect(within(setup).getByRole("textbox", { name: "Secret" })).toHaveValue("ghsec_0123");
    expect(api.sent("POST", "/api/v1/source-connections")[0]!.body).toEqual({ provider: "github", name: "acme/pay", project_keys: ["PAY"] });
  });

  it("lists connections, replaces a secret and disconnects after confirmation", async () => {
    signIn("admin");
    const api = fakeApi(routes([connection({ last_event_at: "2026-01-01T00:00:00Z" })], {
      "POST /api/v1/source-connections/c1/secret": { connection: connection(), secret: "ghsec_new" },
      "DELETE /api/v1/source-connections/c1": () => ({ status: 204 }),
    }));
    vi.spyOn(window, "confirm").mockReturnValue(true);
    show();
    const list = await screen.findByRole("list", { name: "Connections" });
    expect(within(list).getByText(/last delivery/)).toBeInTheDocument();
    await userEvent.click(within(list).getByRole("button", { name: "New secret" }));
    const setup = await screen.findByRole("dialog", { name: "Finish in GitHub" });
    expect(within(setup).getByRole("textbox", { name: "Secret" })).toHaveValue("ghsec_new");
    await userEvent.click(within(setup).getByRole("button", { name: "Done" }));
    await userEvent.click(within(list).getByRole("button", { name: "Disconnect acme/pay" }));
    await waitFor(() => expect(api.sent("DELETE", "/api/v1/source-connections/c1")).toHaveLength(1));
  });
});
