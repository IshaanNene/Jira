import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InvitationsCard } from "@/modules/organizations/components/InvitationsCard";
import { InviteDialog } from "@/modules/organizations/components/InviteDialog";

import { fakeApi, renderAt, signIn, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const at = new Date(Date.now() - 3 * 86_400_000).toISOString();
const invitation = (overrides: Record<string, unknown>) => ({
  id: "inv-1", email: "grace@example.com", org_role: "member", invited_by: "u1", invited_by_name: "Ada Lovelace",
  state: "pending", created_at: at, expires_at: new Date(Date.now() + 4 * 86_400_000).toISOString(), last_sent_at: at,
  ...overrides,
});

describe("invitations", () => {
  it("says when the invitation was emailed, and still offers the link", async () => {
    signIn("owner");
    const api = fakeApi({
      "POST /api/v1/organizations/current/invitations": (call: Call) => ({
        status: 201,
        body: invitation({ ...(call.body as object), last_sent_at: new Date().toISOString(), accept_url: "http://web.test/invite/abc" }),
      }),
    });
    renderAt(<InviteDialog />, "/people", "/people");
    await userEvent.click(screen.getByRole("button", { name: "Invite people" }));
    const dialog = await screen.findByRole("dialog", { name: "Invite people" });
    await userEvent.type(within(dialog).getByLabelText("Email address"), "grace@example.com");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create invitation" }));
    const sent = await screen.findByRole("dialog", { name: "Invitation sent" });
    expect(within(sent).getByText(/We emailed grace@example.com an invitation/)).toBeInTheDocument();
    expect(within(sent).getByDisplayValue("http://web.test/invite/abc")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/organizations/current/invitations")[0]!.body).toEqual({ email: "grace@example.com", org_role: "member" });
  });

  it("sends an invitation again, renews an expired one, and shows the link when it cannot be emailed", async () => {
    signIn("owner");
    let resends = 0;
    const api = fakeApi({
      "GET /api/v1/organizations/current/invitations": {
        items: [invitation({}), invitation({ id: "inv-2", email: "late@example.com", state: "expired", last_sent_at: null })],
      },
      "POST /api/v1/organizations/current/invitations/inv-1/resend": () =>
        ++resends === 1
          ? { body: invitation({ last_sent_at: new Date().toISOString(), accept_url: "http://web.test/invite/new" }) }
          : { status: 429, body: { code: "rate_limited", detail: "Too many requests" } },
      "POST /api/v1/organizations/current/invitations/inv-2/resend": () => ({
        body: invitation({ id: "inv-2", email: "late@example.com", last_sent_at: null, accept_url: "http://web.test/invite/late" }),
      }),
    });
    renderAt(<InvitationsCard />, "/people", "/people");
    expect(await screen.findByText(/emailed 3 days ago/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Send the invitation for grace@example.com again" }));
    expect(await screen.findByText("A new link went to grace@example.com; the old one no longer works.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Send the invitation for grace@example.com again" }));
    expect(await screen.findByText("It was emailed a moment ago. Try again in a minute.")).toBeInTheDocument();

    const renew = screen.getByRole("button", { name: "Send the invitation for late@example.com again" });
    expect(renew).toHaveTextContent("Renew");
    await userEvent.click(renew);
    const dialog = await screen.findByRole("dialog", { name: "New invitation link" });
    expect(within(dialog).getByDisplayValue("http://web.test/invite/late")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/organizations/current/invitations/inv-2/resend")).toHaveLength(1);
  });
});
