import { LogOut, Monitor, Moon, Sun } from "lucide-react";
import { useNavigate } from "react-router-dom";

import {
  Avatar,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui";
import { signOut } from "@/modules/auth";
import { useMyProfile } from "@/modules/users";
import { useSession } from "@/state/session";
import { useUi, type Theme } from "@/state/ui";

import { SETTINGS_NAV } from "./navigation";

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor },
];

export function UserMenu({ collapsed }: { collapsed: boolean }) {
  const navigate = useNavigate();
  const account = useSession((s) => s.account);
  // The profile (editable here) is the source of the display name; the token's copy is a fallback.
  const profile = useMyProfile();
  const { theme, setTheme } = useUi();
  if (!account) return null;
  const name = profile.data?.display_name ?? account.display_name;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex h-10 w-full items-center gap-2 rounded px-1.5 text-left transition-colors hover:bg-sunken" aria-label="Account menu">
        <Avatar name={name} seed={account.id} />
        {!collapsed && (
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-medium text-ink">{name}</span>
            <span className="block truncate text-xs text-ink-subtle">{account.email}</span>
          </span>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-60">
        <div className="px-2 py-1.5">
          <div className="truncate text-sm font-medium text-ink">{name}</div>
          <div className="truncate text-xs text-ink-subtle">{account.email}</div>
        </div>
        <DropdownMenuSeparator />
        {SETTINGS_NAV.filter((item) => !item.requires && !["/settings/workspace", "/settings/workflows"].includes(item.to)).map((item) => (
          <DropdownMenuItem key={item.to} onSelect={() => navigate(item.to)}>
            <item.icon className="h-4 w-4 text-ink-muted" aria-hidden /> {item.label}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Theme</DropdownMenuLabel>
        {THEMES.map((option) => (
          <DropdownMenuItem key={option.value} selected={theme === option.value} onSelect={() => setTheme(option.value)}>
            <option.icon className="h-4 w-4 text-ink-muted" aria-hidden /> {option.label}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void signOut()}>
          <LogOut className="h-4 w-4 text-ink-muted" aria-hidden /> Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
