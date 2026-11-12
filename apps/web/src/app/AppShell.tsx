import { ChevronDown, Search, Settings } from "lucide-react";
import { Suspense } from "react";
import { Link, NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";

import {
  Button,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Skeleton,
  Tooltip,
} from "@/components/ui";
import { CreateIssueDialog } from "@/modules/issues";
import { can } from "@/modules/organizations";
import { CreateProjectDialog, ProjectAvatar, useProjects } from "@/modules/projects";
import { useSession } from "@/state/session";
import { useUi } from "@/state/ui";
import { cn } from "@/utils/cn";
import { PRODUCT_NAME } from "@/utils/config";

import { CommandPalette } from "./CommandPalette";
import { ErrorBoundary } from "./ErrorBoundary";
import { MAIN_NAV, SETTINGS_NAV } from "./navigation";
import { useGlobalShortcuts } from "./useGlobalShortcuts";
import { UserMenu } from "./UserMenu";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

const topLink = (active: boolean) =>
  cn(
    "relative flex h-14 items-center gap-1 px-2.5 text-sm font-medium transition-colors",
    active ? "text-accent after:absolute after:inset-x-1 after:bottom-0 after:h-[3px] after:rounded-t after:bg-accent" : "text-ink-muted hover:text-ink",
  );

function ProjectsMenu() {
  const navigate = useNavigate();
  const location = useLocation();
  const role = useSession((s) => s.organization?.role);
  const projects = useProjects();
  const recent = (projects.data ?? []).slice(0, 6);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className={topLink(location.pathname.startsWith("/projects"))}>
        Projects <ChevronDown className="h-3.5 w-3.5" aria-hidden />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        {recent.length > 0 && <DropdownMenuLabel>Projects</DropdownMenuLabel>}
        {recent.map((p) => (
          <DropdownMenuItem key={p.id} onSelect={() => navigate(`/projects/${p.key}/issues`)}>
            <ProjectAvatar projectKey={p.key} />
            <span className="truncate">{p.name}</span>
            <span className="ml-auto font-mono text-xs text-ink-subtle">{p.key}</span>
          </DropdownMenuItem>
        ))}
        {recent.length > 0 && <DropdownMenuSeparator />}
        <DropdownMenuItem onSelect={() => navigate("/projects")}>View all projects</DropdownMenuItem>
        {can(role, "viewMembers") && (
          // The dialog needs its own trigger element, so the item only opens it.
          <CreateProjectDialog trigger={<DropdownMenuItem onSelect={(e) => e.preventDefault()}>Create project</DropdownMenuItem>} />
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function PageFallback() {
  return (
    <div className="mx-auto w-full max-w-5xl space-y-4 px-6 py-6" aria-busy>
      <Skeleton className="h-7 w-48" />
      <Skeleton className="h-40" />
    </div>
  );
}

/** Persistent frame: top navigation bar, then the routed page (which may add its own sidebar). */
export function AppShell() {
  useGlobalShortcuts();
  const location = useLocation();
  const role = useSession((s) => s.organization?.role);
  const openPalette = useUi((s) => s.setCommandPaletteOpen);
  const openCreate = useUi((s) => s.openCreateIssue);
  const projectKey = /^\/projects\/([^/]+)/.exec(location.pathname)?.[1]?.toUpperCase();

  return (
    <div className="flex h-full flex-col bg-canvas">
      <a href="#main" className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:z-50 focus:rounded focus:bg-surface focus:px-3 focus:py-1.5 focus:shadow-overlay">
        Skip to content
      </a>
      <header className="flex h-14 shrink-0 items-center gap-1 border-b border-line bg-surface px-3">
        <Link to="/" className="mr-1 flex items-center gap-2 rounded px-1.5 py-1" aria-label={`${PRODUCT_NAME} home`}>
          <img src="/mark.svg" alt="" className="h-6 w-6" />
          <span className="hidden text-lg font-semibold tracking-tight text-ink sm:inline">{PRODUCT_NAME}</span>
        </Link>
        <nav className="flex items-center" aria-label="Primary">
          {MAIN_NAV.filter((item) => !item.requires || can(role, item.requires)).map((item) =>
            item.to === "/projects" ? (
              <ProjectsMenu key={item.to} />
            ) : (
              <NavLink key={item.to} to={item.to} end={item.end} className={({ isActive }) => topLink(isActive)}>
                {item.label}
              </NavLink>
            ),
          )}
        </nav>
        <Tooltip label={<>Create issue <kbd className="ml-1 font-mono opacity-70">C</kbd></>} side="bottom">
          <Button variant="primary" className="ml-2" onClick={() => openCreate({ projectKey })}>Create</Button>
        </Tooltip>

        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            onClick={() => openPalette(true)}
            className="mr-1 hidden h-8 w-56 items-center gap-2 rounded border border-line-strong px-2.5 text-sm text-ink-subtle transition-colors hover:bg-sunken md:flex"
          >
            <Search className="h-3.5 w-3.5" aria-hidden />
            <span className="flex-1 truncate text-left">Search</span>
            <kbd className="font-mono text-2xs">{isMac ? "⌘K" : "Ctrl K"}</kbd>
          </button>
          <div className="w-44"><WorkspaceSwitcher collapsed={false} /></div>
          <Tooltip label="Settings" side="bottom">
            <Button asChild variant="ghost" size="icon"><Link to="/settings/workspace" aria-label="Settings"><Settings className="h-4 w-4" /></Link></Button>
          </Tooltip>
          <div className="w-10"><UserMenu collapsed /></div>
        </div>
      </header>

      <main id="main" tabIndex={-1} className="min-h-0 flex-1 overflow-y-auto outline-none">
        <ErrorBoundary resetKey={location.pathname}>
          <Suspense fallback={<PageFallback />}>
            <Outlet />
          </Suspense>
        </ErrorBoundary>
      </main>
      <CommandPalette />
      <CreateIssueDialog />
    </div>
  );
}

/** Settings area: section list on the left, the chosen section on the right. */
export function SettingsLayout() {
  const role = useSession((s) => s.organization?.role);
  return (
    <div className="flex h-full">
      <aside className="hidden w-60 shrink-0 border-r border-line bg-sunken/60 px-2 py-5 md:block" aria-label="Settings">
        <div className="px-3 pb-2 text-2xs font-bold uppercase tracking-wide text-ink-subtle">Settings</div>
        <nav className="space-y-0.5">
          {SETTINGS_NAV.filter((item) => !item.requires || can(role, item.requires)).map((item) => (
            <NavLink key={item.to} to={item.to} end={item.to === "/settings/workflows" ? false : undefined}
              className={({ isActive }) => cn("flex h-9 items-center gap-3 rounded px-3 text-sm font-medium", isActive ? "bg-accent-soft text-accent" : "text-ink-muted hover:bg-sunken hover:text-ink")}>
              <item.icon className="h-4 w-4" aria-hidden /> {item.label}
            </NavLink>
          ))}
        </nav>
      </aside>
      <div className="min-w-0 flex-1 overflow-y-auto">
        <Suspense fallback={<PageFallback />}><Outlet /></Suspense>
      </div>
    </div>
  );
}
