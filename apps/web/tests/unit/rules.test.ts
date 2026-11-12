import { describe, expect, it } from "vitest";

import { safeNext } from "@/modules/auth/navigation";
import { slugPreview } from "@/modules/organizations/components/CreateOrganizationForm";
import { assignableRoles, can, invitableRoles } from "@/modules/organizations/permissions";
import { describeUserAgent, formatRelative, initials } from "@/utils/format";

describe("safeNext", () => {
  it("allows only same-app paths", () => {
    expect(safeNext("/people?x=1")).toBe("/people?x=1");
    for (const bad of [null, "", "https://evil.example", "//evil.example", "/\\evil.example", "javascript:alert(1)"]) {
      expect(safeNext(bad)).toBe("/");
    }
  });
});

describe("organization permissions (display only; server enforces)", () => {
  it("matches the server's role table", () => {
    expect(can("guest", "viewMembers")).toBe(false);
    expect(can("member", "viewMembers")).toBe(true);
    expect(can("member", "inviteMembers")).toBe(false);
    expect(can("admin", "manageMembers")).toBe(true);
    expect(can("admin", "manageOwnership")).toBe(false);
    expect(can("owner", "manageOwnership")).toBe(true);
    expect(can(undefined, "viewMembers")).toBe(false);
  });
  it("never offers an admin a way to touch owners", () => {
    expect(assignableRoles("admin", "owner")).toEqual([]);
    expect(assignableRoles("admin", "member")).not.toContain("owner");
    expect(assignableRoles("owner", "member")).toContain("owner");
    expect(assignableRoles("member", "guest")).toEqual([]);
  });
  it("never offers ownership by invitation", () => {
    expect(invitableRoles("owner")).not.toContain("owner");
    expect(invitableRoles("member")).toEqual([]);
  });
});

describe("formatting", () => {
  it("derives slugs like the server does", () => {
    expect(slugPreview("Acme Rockets, Inc.")).toBe("acme-rockets-inc");
    expect(slugPreview("A")).toBe("a00");
    expect(slugPreview("")).toBe("");
  });
  it("formats initials, relative time and user agents", () => {
    expect(initials("Ada Lovelace")).toBe("AL");
    expect(initials("ada")).toBe("A");
    const now = new Date("2026-11-11T12:00:00Z");
    expect(formatRelative("2026-11-18T11:59:50Z", now)).toMatch(/7 days/);
    expect(formatRelative("2026-11-11T09:00:00Z", now)).toMatch(/3 hours ago/);
    expect(describeUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/130.0 Safari/537.36")).toBe("Chrome on macOS");
    expect(describeUserAgent(null)).toBe("Unknown device");
  });
});
