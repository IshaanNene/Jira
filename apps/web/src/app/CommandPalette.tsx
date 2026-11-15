import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Command } from "cmdk";
import { Building2, LogOut, Moon, Plus, Search, Sun } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";

import { signOut, switchOrganization, twoStepRequiredPath } from "@/modules/auth";
import { can, useMyOrganizations } from "@/modules/organizations";
import { IssueTypeIcon } from "@/modules/issues";
import { ProjectAvatar, useProjects } from "@/modules/projects";
import { Highlighted, hitPath, useDebounced, useQuickSearch } from "@/modules/search";
import { useSession } from "@/state/session";
import { useUi } from "@/state/ui";

import { ALL_NAV } from "./navigation";

const itemClass =
  "flex h-9 cursor-default select-none items-center gap-2.5 rounded px-2.5 text-sm text-ink data-[selected=true]:bg-sunken [&_svg]:h-4 [&_svg]:w-4 [&_svg]:text-ink-muted";
const groupClass = "[&_[cmdk-group-heading]]:px-2.5 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-2xs [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wide [&_[cmdk-group-heading]]:text-ink-subtle";

/** ⌘K: jump anywhere and run common actions from the keyboard. */
export function CommandPalette() {
  const navigate = useNavigate();
  const open = useUi((s) => s.commandPaletteOpen);
  const setOpen = useUi((s) => s.setCommandPaletteOpen);
  const { theme, setTheme } = useUi();
  const role = useSession((s) => s.organization?.role);
  const activeId = useSession((s) => s.organization?.id);
  const organizations = useMyOrganizations();
  const projects = useProjects();
  const openCreate = useUi((s) => s.openCreateIssue);
  const isDark = document.documentElement.classList.contains("dark");
  const [text, setText] = useState("");
  const q = useDebounced(text.trim(), 200);
  const issues = useQuickSearch(q, open);
  const hits = q.length >= 2 ? (issues.data?.items ?? []) : [];

  const run = (action: () => void | Promise<void>) => {
    setOpen(false);
    void action();
  };

  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setText("");
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-ink/40 data-[state=open]:animate-fade-in" />
        <DialogPrimitive.Content
          aria-describedby={undefined}
          className="fixed left-1/2 top-[14%] z-50 w-[calc(100vw-2rem)] max-w-xl -translate-x-1/2 overflow-hidden rounded-xl border border-line bg-raised shadow-overlay data-[state=open]:animate-pop-in"
        >
          <DialogPrimitive.Title className="sr-only">Command palette</DialogPrimitive.Title>
          <Command label="Command palette" loop>
            <div className="flex items-center gap-2 border-b border-line px-3">
              <Search className="h-4 w-4 shrink-0 text-ink-subtle" aria-hidden />
              <Command.Input autoFocus value={text} onValueChange={setText} maxLength={200} placeholder="Search issues, pages and actions…" className="h-11 w-full bg-transparent text-base text-ink outline-none placeholder:text-ink-subtle focus-visible:ring-0" />
            </div>
            <Command.List className="max-h-80 overflow-y-auto p-1.5">
              {/* With a query the issue group below always offers "Search all issues". */}
              {q.length < 2 && <Command.Empty className="px-3 py-8 text-center text-sm text-ink-muted">Nothing matches. Try a different word.</Command.Empty>}
              {q.length >= 2 && (
                // Matched by the server, so cmdk's own filter must not hide them.
                <Command.Group heading="Issues" className={groupClass} forceMount>
                  {hits.map((hit) => (
                    <Command.Item key={hit.id} value={`issue ${hit.key}`} forceMount onSelect={() => run(() => navigate(hitPath(hit)))} className={itemClass}>
                      <IssueTypeIcon type={hit.type} />
                      <span className="shrink-0 text-ink-muted">{hit.key}</span>
                      <span className="truncate">
                        <Highlighted parts={hit.summary_parts} fallback={hit.summary} />
                      </span>
                    </Command.Item>
                  ))}
                  <Command.Item
                    value="search all issues"
                    forceMount
                    onSelect={() => run(() => navigate(`/search?q=${encodeURIComponent(q)}`))}
                    className={itemClass}
                  >
                    <Search aria-hidden /> {hits.length ? "View all results" : issues.isFetching ? "Searching issues…" : "Search all issues"} for “{q}”
                  </Command.Item>
                </Command.Group>
              )}
              <Command.Group heading="Go to" className={groupClass}>
                {ALL_NAV.filter((item) => !item.requires || can(role, item.requires)).map((item) => (
                  <Command.Item key={item.to} value={`go ${item.label}`} onSelect={() => run(() => navigate(item.to))} className={itemClass}>
                    <item.icon aria-hidden /> {item.label}
                    {item.shortcut && <kbd className="ml-auto font-mono text-2xs text-ink-subtle">G {item.shortcut.toUpperCase()}</kbd>}
                  </Command.Item>
                ))}
              </Command.Group>
              {(projects.data?.length ?? 0) > 0 && (
                <Command.Group heading="Projects" className={groupClass}>
                  {projects.data?.map((p) => (
                    <Command.Item key={p.id} value={`project ${p.name} ${p.key}`} onSelect={() => run(() => navigate(`/projects/${p.key}/issues`))} className={itemClass}>
                      <ProjectAvatar projectKey={p.key} size="sm" /> {p.name}
                      <span className="ml-auto font-mono text-2xs text-ink-subtle">{p.key}</span>
                    </Command.Item>
                  ))}
                </Command.Group>
              )}
              {(organizations.data?.length ?? 0) > 1 && (
                <Command.Group heading="Switch workspace" className={groupClass}>
                  {organizations.data
                    ?.filter((org) => org.id !== activeId)
                    .map((org) => (
                      <Command.Item
                        key={org.id}
                        value={`switch workspace ${org.name} ${org.slug}`}
                        onSelect={() =>
                          run(async () => {
                            try {
                              await switchOrganization(org.id);
                              navigate("/");
                            } catch (error) {
                              navigate(twoStepRequiredPath(error, org.id) ?? "/onboarding");
                            }
                          })
                        }
                        className={itemClass}
                      >
                        <Building2 aria-hidden /> {org.name}
                      </Command.Item>
                    ))}
                </Command.Group>
              )}
              <Command.Group heading="Actions" className={groupClass}>
                <Command.Item value="create issue new" onSelect={() => run(() => openCreate())} className={itemClass}>
                  <Plus aria-hidden /> Create issue
                  <kbd className="ml-auto font-mono text-2xs text-ink-subtle">C</kbd>
                </Command.Item>
                <Command.Item value="toggle theme dark light appearance" onSelect={() => run(() => setTheme(isDark ? "light" : "dark"))} className={itemClass}>
                  {isDark ? <Sun aria-hidden /> : <Moon aria-hidden />} Switch to {isDark ? "light" : "dark"} theme
                  {theme === "system" && <span className="ml-auto text-2xs text-ink-subtle">Following system</span>}
                </Command.Item>
                <Command.Item value="sign out log out" onSelect={() => run(signOut)} className={itemClass}>
                  <LogOut aria-hidden /> Sign out
                </Command.Item>
              </Command.Group>
            </Command.List>
          </Command>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
