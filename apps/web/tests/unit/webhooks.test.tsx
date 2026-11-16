import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import WebhooksPage from "@/modules/integrations/pages/WebhooksPage";
import type { Webhook } from "@/services/api";

import { fakeApi, project, renderAt, signIn, USER_ID } from "./harness";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const hook = (overrides: Partial<Webhook> = {}): Webhook => ({
  id: "w1",
  name: "Deploy bot",
  kind: "webhook",
  url: "https://hooks.example.com/meridian",
  events: ["issue.created"],
  project_ids: [],
  enabled: true,
  disabled_reason: null,
  consecutive_failures: 0,
  created_by: USER_ID,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  ...overrides,
});

const EVENTS = { items: [{ name: "issue.created", description: "An issue was created" }, { name: "comment.created", description: "A comment was added" }] };

const routes = (hooks: Webhook[], extra: Record<string, unknown> = {}) => ({
  "GET /api/v1/webhooks": { items: hooks },
  "GET /api/v1/webhooks/events": EVENTS,
  "GET /api/v1/projects": { items: [project()] },
  ...extra,
});

const show = () => renderAt(<WebhooksPage />, "/settings/webhooks", "/settings/webhooks");

describe("webhooks", () => {
  it("is for admins only", async () => {
    signIn("member");
    fakeApi(routes([]));
    show();
    expect(await screen.findByText("Admins only")).toBeInTheDocument();
  });

  it("creates a webhook and shows its secret once", async () => {
    signIn("admin");
    const api = fakeApi(routes([], {
      "POST /api/v1/webhooks": () => ({ status: 201, body: { webhook: hook({ name: "CI" }), secret: "whsec_0123" } }),
    }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: /new webhook/i }));
    const dialog = await screen.findByRole("dialog", { name: "New webhook" });
    await userEvent.type(within(dialog).getByLabelText("Name"), "CI");
    const url = within(dialog).getByLabelText(/Endpoint URL/);
    await userEvent.clear(url);
    await userEvent.type(url, "https://ci.example.com/hook");
    await userEvent.click(await within(dialog).findByRole("checkbox", { name: /comment\.created/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Create webhook" }));
    const secret = await screen.findByRole("dialog", { name: "Signing secret" });
    expect(within(secret).getByRole("textbox", { name: "Signing secret" })).toHaveValue("whsec_0123");
    expect(api.sent("POST", "/api/v1/webhooks")[0]!.body).toEqual({
      name: "CI",
      url: "https://ci.example.com/hook",
      events: ["issue.created", "comment.created"],
      project_ids: [],
      kind: "webhook",
    });
  });

  it("connects a Slack channel without a secret, and never shows its URL back", async () => {
    signIn("admin");
    const slack = hook({ id: "s1", name: "Team channel", kind: "slack", url: "https://hooks.slack.com/…WXYZ" });
    const api = fakeApi(routes([], {
      "POST /api/v1/webhooks": () => ({ status: 201, body: { webhook: slack, secret: null } }),
    }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: /new webhook/i }));
    const dialog = await screen.findByRole("dialog", { name: "New webhook" });
    await userEvent.click(within(dialog).getByRole("radio", { name: "Slack channel" }));
    await userEvent.type(within(dialog).getByLabelText("Name"), "Team channel");
    await userEvent.type(within(dialog).getByLabelText(/Slack webhook URL/), "https://hooks.slack.com/services/T0/B0/abcWXYZ");
    await userEvent.click(within(dialog).getByRole("button", { name: "Connect channel" }));
    await waitFor(() => expect(api.sent("POST", "/api/v1/webhooks")).toHaveLength(1));
    expect(api.sent("POST", "/api/v1/webhooks")[0]!.body).toMatchObject({ kind: "slack", url: "https://hooks.slack.com/services/T0/B0/abcWXYZ" });
    expect(screen.queryByRole("dialog", { name: "Signing secret" })).not.toBeInTheDocument();
  });

  it("marks Slack channels and offers no signing secret for them", async () => {
    signIn("admin");
    fakeApi(routes([hook({ kind: "slack", url: "https://hooks.slack.com/…WXYZ" })]));
    show();
    const list = await screen.findByRole("list", { name: "Webhooks" });
    expect(within(list).getByText("Slack")).toBeInTheDocument();
    expect(within(list).queryByRole("button", { name: "New secret" })).not.toBeInTheDocument();
  });

  it("explains why the service turned a webhook off, and turns it back on", async () => {
    signIn("admin");
    const api = fakeApi(routes([hook({ enabled: false, disabled_reason: "The endpoint answered 410 Gone." })], {
      "PATCH /api/v1/webhooks/w1": hook(),
    }));
    show();
    const list = await screen.findByRole("list", { name: "Webhooks" });
    expect(within(list).getByText("The endpoint answered 410 Gone.")).toBeInTheDocument();
    expect(within(list).getByRole("button", { name: "Send a test to Deploy bot" })).toBeDisabled();
    await userEvent.click(within(list).getByRole("button", { name: "Turn on" }));
    expect(api.sent("PATCH", "/api/v1/webhooks/w1")[0]!.body).toEqual({ enabled: true });
  });

  it("shows server refusals in the form", async () => {
    signIn("admin");
    fakeApi(routes([], {
      "POST /api/v1/webhooks": () => ({
        status: 422,
        body: { type: "about:blank", title: "Validation failed", status: 422, code: "validation_failed", detail: "Webhooks can only be delivered to public addresses.", errors: [] },
      }),
    }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: /new webhook/i }));
    const dialog = await screen.findByRole("dialog", { name: "New webhook" });
    await userEvent.type(within(dialog).getByLabelText("Name"), "x");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create webhook" }));
    expect(await within(dialog).findByRole("alert")).toBeInTheDocument();
  });

  it("lists deliveries with their outcome", async () => {
    signIn("admin");
    fakeApi(routes([hook()], {
      "GET /api/v1/webhooks/w1/deliveries": {
        items: [
          { id: "d1", event_id: "e1", event_type: "issue.created", status: "pending", attempts: 2, response_status: 500, last_error: "The endpoint answered 500 Internal Server Error.", created_at: "2026-01-01T00:00:00Z", finished_at: null },
          { id: "d2", event_id: "e2", event_type: "webhook.ping", status: "delivered", attempts: 1, response_status: 204, last_error: null, created_at: "2026-01-01T00:00:00Z", finished_at: "2026-01-01T00:00:01Z" },
        ],
      },
    }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Deliveries" }));
    const log = await screen.findByRole("list", { name: "Deliveries" });
    expect(within(log).getByText("Retrying")).toBeInTheDocument();
    expect(within(log).getByText("HTTP 500")).toBeInTheDocument();
    expect(within(log).getByText("Delivered")).toBeInTheDocument();
  });

  it("replaces the secret only after confirmation", async () => {
    signIn("admin");
    const api = fakeApi(routes([hook()], { "POST /api/v1/webhooks/w1/secret": { webhook: hook(), secret: "whsec_new" } }));
    const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
    show();
    const button = await screen.findByRole("button", { name: "New secret" });
    await userEvent.click(button);
    expect(api.sent("POST", "/api/v1/webhooks/w1/secret")).toHaveLength(0);
    await userEvent.click(button);
    const dialog = await screen.findByRole("dialog", { name: "Signing secret" });
    expect(within(dialog).getByRole("textbox", { name: "Signing secret" })).toHaveValue("whsec_new");
    expect(confirm).toHaveBeenCalledTimes(2);
  });
});
