import { NavLink } from "react-router-dom";

import { cn } from "@/utils/cn";

/** Route-backed tabs: each tab is a link, so the selected tab survives reload and sharing. */
export function RouteTabs({ tabs }: { tabs: { to: string; label: string; end?: boolean }[] }) {
  return (
    <nav className="mb-5 flex gap-1 border-b border-line" aria-label="Sections">
      {tabs.map((tab) => (
        <NavLink
          key={tab.to}
          to={tab.to}
          end={tab.end}
          className={({ isActive }) =>
            cn(
              "-mb-px border-b-2 px-3 py-2 text-sm font-medium transition-colors",
              isActive ? "border-accent text-accent" : "border-transparent text-ink-muted hover:text-ink",
            )
          }
        >
          {tab.label}
        </NavLink>
      ))}
    </nav>
  );
}
