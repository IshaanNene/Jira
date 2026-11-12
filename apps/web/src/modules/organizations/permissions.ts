import type { OrgRole } from "@/services/api";

/**
 * Client-side mirror of `auth_middleware::authz` — used only to decide what to *show*.
 * The server enforces every one of these rules again; hiding a control is not security.
 */
export type OrgPermission = "viewMembers" | "inviteMembers" | "manageMembers" | "manageOrganization" | "manageOwnership";

const RANK: Record<OrgRole, number> = { guest: 0, member: 1, admin: 2, owner: 3 };

const MINIMUM: Record<OrgPermission, OrgRole> = {
  viewMembers: "member",
  inviteMembers: "admin",
  manageMembers: "admin",
  manageOrganization: "admin",
  manageOwnership: "owner",
};

export function can(role: OrgRole | undefined, permission: OrgPermission): boolean {
  return role !== undefined && RANK[role] >= RANK[MINIMUM[permission]];
}

export const ROLE_LABEL: Record<OrgRole, string> = { owner: "Owner", admin: "Admin", member: "Member", guest: "Guest" };

export const ROLE_DESCRIPTION: Record<OrgRole, string> = {
  owner: "Full control, including ownership and deletion",
  admin: "Manage people, settings and all projects",
  member: "Work in projects they are added to",
  guest: "Limited access to what is shared with them",
};

/** Roles `actor` may assign to someone whose current role is `target`. */
export function assignableRoles(actor: OrgRole | undefined, target: OrgRole): OrgRole[] {
  if (!can(actor, "manageMembers")) return [];
  if (actor === "owner") return ["owner", "admin", "member", "guest"];
  // Admins cannot touch owners or grant ownership.
  return target === "owner" ? [] : ["admin", "member", "guest"];
}

/** Roles `actor` may invite someone as. Ownership is never granted by invitation. */
export function invitableRoles(actor: OrgRole | undefined): OrgRole[] {
  return can(actor, "inviteMembers") ? ["member", "admin", "guest"] : [];
}
