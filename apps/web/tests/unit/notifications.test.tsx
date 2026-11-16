import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { badgeLabel, describe as describeNotification, issuePath, NotificationBell } from "@/modules/notifications";
import type { AppNotification } from "@/services/api";

import { fakeApi, renderAt, signIn, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const note = (overrides: Partial<AppNotification> = {}): AppNotification => ({
  id: "n1", kind: "assigned", actor_id: "u2", actor_name: "Maya Patel", created_at: new Date().toISOString(), detail: {},
  issue_id: "i1", issue_key: "PAY-12", issue_summary: "Refunds fail", project_id: "p1", read: false, ...overrides,
});

describe("notification wording", () => {
  it("says who did what", () => {
    expect(describeNotification(note())).toBe("Maya Patel assigned this to you");
    expect(describeNotification(note({ kind: "mentioned" }))).toBe("Maya Patel mentioned you in a comment");
    expect(describeNotification(note({ kind: "mentioned", detail: { in: "description" } as never }))).toBe("Maya Patel mentioned you in the description");
    expect(describeNotification(note({ kind: "status_changed", detail: { to_status: "in_review" } as never }))).toBe(
      "Maya Patel changed the status to In review",
    );
    expect(describeNotification(note({ kind: "status_changed" }))).toBe("Maya Patel changed the status");
    expect(describeNotification(note({ kind: "commented", actor_name: "" }))).toBe("Someone commented");
  });

  it("links to the issue and caps the badge", () => {
    expect(issuePath("PAY-12")).toBe("/projects/PAY/issues/PAY-12");
    expect(issuePath("WEB2-3")).toBe("/projects/WEB2/issues/WEB2-3");
    expect([badgeLabel(0), badgeLabel(3), badgeLabel(12)]).toEqual([null, "3", "9+"]);
  });
});

describe("notification bell", () => {
  it("shows the unread count, lists notifications and opens the issue, marking it read", async () => {
    signIn();
    let unread = 2;
    const api = fakeApi({
      "GET /api/v1/notifications/unread-count": () => ({ body: { count: unread } }),
      "GET /api/v1/notifications": () => ({
        body: { items: [note(), note({ id: "n2", kind: "commented", issue_key: "PAY-9", issue_summary: "Old one", read: true })], unread_count: unread },
      }),
      "POST /api/v1/notifications/n1/read": () => ((unread = 1), { status: 204 }),
      "POST /api/v1/notifications/read-all": () => ((unread = 0), { status: 204 }),
    });
    renderAt(<NotificationBell />, "/", "/");

    const bell = await screen.findByRole("button", { name: "Notifications, 2 unread" });
    expect(bell).toHaveTextContent("2");
    await userEvent.click(bell);
    expect(await screen.findByText("Maya Patel assigned this to you")).toBeInTheDocument();
    expect(screen.getByText("Old one")).toBeInTheDocument();

    await userEvent.click(screen.getByText("Maya Patel assigned this to you"));
    expect(await screen.findByText("other page")).toBeInTheDocument();
    await waitFor(() => expect(api.sent("POST", "/api/v1/notifications/n1/read")).toHaveLength(1));
  });

  it("filters to unread and marks everything read", async () => {
    signIn();
    let unread = 1;
    const api = fakeApi({
      "GET /api/v1/notifications/unread-count": () => ({ body: { count: unread } }),
      "GET /api/v1/notifications": () => ({ body: { items: unread ? [note()] : [], unread_count: unread } }),
      "POST /api/v1/notifications/read-all": () => ((unread = 0), { status: 204 }),
    });
    renderAt(<NotificationBell />, "/", "/");
    await userEvent.click(await screen.findByRole("button", { name: "Notifications, 1 unread" }));
    await userEvent.click(await screen.findByLabelText("Only show unread"));
    await waitFor(() => expect(api.calls.some((c) => c.path === "/api/v1/notifications" && c.query.get("unread") === "true")).toBe(true));

    await userEvent.click(await screen.findByRole("button", { name: /Mark all as read/ }));
    expect(await screen.findByText("No unread notifications")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/notifications/read-all")).toHaveLength(1);
    // The badge clears once the count is refetched.
    expect(await screen.findByRole("button", { name: "Notifications", hidden: true })).toBeInTheDocument();
  });
});

describe("notification settings", () => {
  it("saves each switch at once and puts it back when saving fails", async () => {
    signIn();
    const { NotificationSettingsPage } = await import("@/modules/notifications");
    let fail = false;
    const api = fakeApi({
      "GET /api/v1/notifications/preferences": {
        email_enabled: true, email_assigned: true, email_mentioned: true, email_commented: true, email_status_changed: true,
      },
      "PUT /api/v1/notifications/preferences": (call: Call) => (fail ? { status: 503, body: { code: "unavailable" } } : { body: call.body }),
    });
    renderAt(<NotificationSettingsPage />, "/settings/notifications", "/settings/notifications");

    const comments = await screen.findByRole("switch", { name: "Comments" });
    await userEvent.click(comments);
    await waitFor(() => expect(api.sent("PUT", "/api/v1/notifications/preferences")).toHaveLength(1));
    expect(api.sent("PUT", "/api/v1/notifications/preferences")[0]!.body).toMatchObject({ email_enabled: true, email_commented: false });
    expect(comments).toHaveAttribute("aria-checked", "false");
    // A server that leaves the digest setting out means it is on.
    const digests = screen.getByRole("switch", { name: "Saved filter subscriptions" });
    expect(digests).toHaveAttribute("aria-checked", "true");
    await userEvent.click(digests);
    await waitFor(() => expect(api.sent("PUT", "/api/v1/notifications/preferences")).toHaveLength(2));
    expect(api.sent("PUT", "/api/v1/notifications/preferences")[1]!.body).toMatchObject({ email_filter_digest: false, email_commented: false });

    fail = true;
    const master = screen.getByRole("switch", { name: "Send me email notifications" });
    await userEvent.click(master);
    await waitFor(() => expect(api.sent("PUT", "/api/v1/notifications/preferences")).toHaveLength(3));
    await waitFor(() => expect(master).toHaveAttribute("aria-checked", "true"));
    expect(await screen.findByText("Couldn't save your notification settings. Try again.")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Status changes" })).toBeEnabled();
  });

  it("chooses a digest and resumes email paused by a bounce", async () => {
    signIn();
    const { NotificationSettingsPage } = await import("@/modules/notifications");
    let paused: string | null = "bounce";
    const api = fakeApi({
      "GET /api/v1/notifications/preferences": () => ({
        body: {
          email_enabled: true, email_assigned: true, email_mentioned: true, email_commented: true, email_status_changed: true,
          email_digest: "off", email_paused: paused,
        },
      }),
      "PUT /api/v1/notifications/preferences": (call: Call) => ({ body: call.body }),
      "DELETE /api/v1/notifications/preferences/email-pause": () => {
        paused = null;
        return { status: 204, body: null };
      },
    });
    renderAt(<NotificationSettingsPage />, "/settings/notifications", "/settings/notifications");

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Email is paused");
    expect(alert).toHaveTextContent("the mailbox doesn't exist");
    await userEvent.click(screen.getByRole("button", { name: "Resume email" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(api.sent("DELETE", "/api/v1/notifications/preferences/email-pause")).toHaveLength(1);

    expect(screen.getByRole("radio", { name: "As it happens" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("radio", { name: "Daily digest" }));
    await waitFor(() => expect(api.sent("PUT", "/api/v1/notifications/preferences")).toHaveLength(1));
    expect(api.sent("PUT", "/api/v1/notifications/preferences")[0]!.body).toMatchObject({ email_digest: "daily" });
    expect(screen.getByRole("radio", { name: "Daily digest" })).toHaveAttribute("aria-checked", "true");
  });
});

describe("unsubscribe link", () => {
  it("asks before turning anything off, then confirms what was turned off", async () => {
    const { UnsubscribePage } = await import("@/modules/notifications");
    const api = fakeApi({
      "GET /api/v1/notifications/unsubscribe": { kind: "commented", kind_label: "comments" },
      "POST /api/v1/notifications/unsubscribe": () => ({ status: 204 }),
    });
    renderAt(<UnsubscribePage />, "/unsubscribe", "/unsubscribe?token=abc.def");

    const stop = await screen.findByRole("button", { name: "Stop emails about comments" });
    expect(api.sent("POST", "/api/v1/notifications/unsubscribe")).toHaveLength(0);
    expect(api.calls[0]!.headers.Authorization).toBeUndefined();
    await userEvent.click(stop);
    expect(await screen.findByText("You're unsubscribed")).toBeInTheDocument();
    const sent = api.sent("POST", "/api/v1/notifications/unsubscribe")[0]!;
    expect(sent.body).toEqual({});
    expect(sent.query.get("token")).toBe("abc.def");
    expect(screen.getByText("You won't get emails about comments any more.")).toBeInTheDocument();
  });

  it("explains a link that no longer works", async () => {
    fakeApi({ "GET /api/v1/notifications/unsubscribe": () => ({ status: 404, body: { code: "not_found" } }) });
    const { UnsubscribePage } = await import("@/modules/notifications");
    renderAt(<UnsubscribePage />, "/unsubscribe", "/unsubscribe?token=old");
    expect(await screen.findByText("This unsubscribe link doesn't work")).toBeInTheDocument();
  });
});
