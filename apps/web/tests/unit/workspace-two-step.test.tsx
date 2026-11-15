import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { twoStepRequiredPath } from "@/modules/auth";
import { OrganizationSettingsPage } from "@/modules/organizations";
import TwoStepRequiredPage from "@/modules/users/pages/TwoStepRequiredPage";
import { ApiError } from "@/services/api";
import { useSession } from "@/state/session";

import { fakeApi, ORG_ID, renderAt, signIn, USER_ID, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const org = (overrides: Record<string, unknown> = {}) => ({
  id: ORG_ID,
  name: "Acme",
  slug: "acme",
  role: "admin",
  require_two_step: false,
  version: 1,
  created_at: "2026-01-01T00:00:00Z",
  ...overrides,
});
const mfa = (on: boolean) => ({ totp_enabled: on, enabled_at: on ? "2026-11-01T00:00:00Z" : null, recovery_codes_remaining: on ? 10 : 0 });

describe("requiring two-step verification in a workspace", () => {
  it("lets an admin who has it turn the requirement on", async () => {
    signIn("admin");
    let current = org();
    const api = fakeApi({
      "GET /api/v1/organizations/current": () => ({ body: current }),
      "GET /api/v1/auth/mfa": mfa(true),
      "PATCH /api/v1/organizations/current": (call: Call) => {
        current = org({ ...(call.body as object), version: 2 });
        return { body: current };
      },
    });
    renderAt(<OrganizationSettingsPage />, "/settings/workspace", "/settings/workspace");

    expect(await screen.findByText("Optional")).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "Require for everyone" }));
    expect(await screen.findByText("Required")).toBeInTheDocument();
    const sent = api.sent("PATCH", "/api/v1/organizations/current")[0]!;
    expect(sent.body).toEqual({ require_two_step: true });
    expect(sent.headers["If-Match"]).toBe('"1"');
    expect(screen.getByRole("button", { name: "Stop requiring" })).toBeInTheDocument();
  });

  it("asks an admin without it to turn it on for themselves first; members only read it", async () => {
    signIn("admin");
    fakeApi({ "GET /api/v1/organizations/current": org(), "GET /api/v1/auth/mfa": mfa(false) });
    const { unmount } = renderAt(<OrganizationSettingsPage />, "/settings/workspace", "/settings/workspace");
    expect(await screen.findByText("Turn it on for your own account first, in Security settings.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Require for everyone" })).toBeDisabled();
    unmount();

    signIn("member");
    fakeApi({ "GET /api/v1/organizations/current": org({ require_two_step: true, role: "member" }), "GET /api/v1/auth/mfa": mfa(false) });
    renderAt(<OrganizationSettingsPage />, "/settings/workspace", "/settings/workspace");
    expect(await screen.findByText("Required")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop requiring" })).toBeNull();
  });

  it("sends a refused member to set it up, then into the workspace", async () => {
    const refusal = new ApiError(403, { code: "two_step_required", detail: "This workspace requires two-step verification." } as never);
    expect(twoStepRequiredPath(refusal, ORG_ID)).toBe(`/two-step-required?workspace=${ORG_ID}`);
    expect(twoStepRequiredPath(new ApiError(403, null), ORG_ID)).toBeNull();

    signIn("member");
    useSession.setState({ organization: null });
    let on = false;
    const api = fakeApi({
      "GET /api/v1/organizations": { items: [org({ role: "member", require_two_step: true })] },
      "GET /api/v1/auth/mfa": () => ({ body: mfa(on) }),
      "POST /api/v1/auth/token/exchange": {
        access_token: "org-token",
        token_type: "Bearer",
        expires_in: 600,
        account: { id: USER_ID, email: "ada@example.com", display_name: "Ada Lovelace", email_verified: true, created_at: "2026-01-01T00:00:00Z" },
        organization: { id: ORG_ID, role: "member" },
      },
    });
    const { unmount } = renderAt(<TwoStepRequiredPage />, "/two-step-required", `/two-step-required?workspace=${ORG_ID}`);

    expect(await screen.findByText(/Acme requires it of everyone/)).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Set up" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue to Acme" })).toBeDisabled();
    unmount();

    on = true;
    renderAt(<TwoStepRequiredPage />, "/two-step-required", `/two-step-required?workspace=${ORG_ID}`);
    const proceed = await screen.findByRole("button", { name: "Continue to Acme" });
    await waitFor(() => expect(proceed).toBeEnabled());
    await userEvent.click(proceed);
    await waitFor(() => expect(useSession.getState().organization?.id).toBe(ORG_ID));
    expect(api.sent("POST", "/api/v1/auth/token/exchange")[0]!.body).toEqual({ organization_id: ORG_ID });
  });
});
