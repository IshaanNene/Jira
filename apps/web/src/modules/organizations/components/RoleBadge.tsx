import { Badge } from "@/components/ui";
import type { OrgRole } from "@/services/api";

import { ROLE_LABEL } from "../permissions";

export function RoleBadge({ role }: { role: string }) {
  const known = role in ROLE_LABEL ? (role as OrgRole) : undefined;
  return <Badge tone={known === "owner" ? "accent" : "neutral"}>{known ? ROLE_LABEL[known] : role}</Badge>;
}
