import type { ReactNode } from "react";
import { Link, NavLink } from "react-router-dom";

import { Button } from "@/components/ui";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";
import { PRODUCT_NAME } from "@/utils/config";

const NAV = [
  { to: "/features", label: "Features" },
  { to: "/pricing", label: "Pricing" },
  { to: "/security", label: "Security" },
];

/** The public site's frame: header with sign-in, content, footer. */
export function MarketingLayout({ children }: { children: ReactNode }) {
  const signedIn = useSession((s) => s.status === "authenticated");
  const year = new Date().getFullYear();
  return (
    <div className="min-h-full bg-canvas text-ink">
      <a href="#main" className="sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-4 focus:z-50 focus:rounded focus:bg-surface focus:px-3 focus:py-2">
        Skip to content
      </a>
      <header className="sticky top-0 z-40 border-b border-line bg-canvas/90 backdrop-blur">
        <div className="mx-auto flex h-14 max-w-6xl items-center gap-6 px-4">
          <Link to="/" className="flex items-center gap-2" aria-label={`${PRODUCT_NAME} home`}>
            <img src="/mark.svg" alt="" className="h-7 w-7" />
            <span className="text-lg font-semibold tracking-tight">{PRODUCT_NAME}</span>
          </Link>
          <nav aria-label="Site" className="hidden items-center gap-1 sm:flex">
            {NAV.map((n) => (
              <NavLink
                key={n.to}
                to={n.to}
                className={({ isActive }) => cn("rounded px-2.5 py-1.5 text-sm font-medium text-ink-muted hover:bg-sunken hover:text-ink", isActive && "text-ink")}
              >
                {n.label}
              </NavLink>
            ))}
          </nav>
          <div className="ml-auto flex items-center gap-2">
            {signedIn ? (
              <Button asChild variant="primary"><Link to="/">Open your workspace</Link></Button>
            ) : (
              <>
                <Button asChild variant="ghost"><Link to="/login">Sign in</Link></Button>
                <Button asChild variant="primary"><Link to="/register">Get started free</Link></Button>
              </>
            )}
          </div>
        </div>
        <nav aria-label="Site (small screens)" className="flex gap-1 overflow-x-auto px-4 pb-2 sm:hidden">
          {NAV.map((n) => (
            <NavLink key={n.to} to={n.to} className="rounded px-2.5 py-1 text-sm font-medium text-ink-muted hover:bg-sunken">{n.label}</NavLink>
          ))}
        </nav>
      </header>
      <main id="main">{children}</main>
      <footer className="border-t border-line bg-sunken">
        <div className="mx-auto grid max-w-6xl gap-8 px-4 py-10 sm:grid-cols-4">
          <div>
            <div className="flex items-center gap-2">
              <img src="/mark.svg" alt="" className="h-6 w-6" />
              <span className="font-semibold">{PRODUCT_NAME}</span>
            </div>
            <p className="mt-2 text-sm text-ink-muted">Plan, track and ship work with your whole team.</p>
          </div>
          <FooterColumn title="Product" links={[["/features", "Features"], ["/pricing", "Pricing"], ["/security", "Security"]]} />
          <FooterColumn title="Get started" links={signedIn ? [["/", "Open your workspace"]] : [["/register", "Create a workspace"], ["/login", "Sign in"]]} />
          <FooterColumn title="Features" links={[["/features#agile", "Boards and sprints"], ["/features#workflows", "Workflows"], ["/features#dashboards", "Dashboards"]]} />
        </div>
        <p className="mx-auto max-w-6xl px-4 pb-8 text-xs text-ink-subtle">© {year} {PRODUCT_NAME}. Preview release.</p>
      </footer>
    </div>
  );
}

function FooterColumn({ title, links }: { title: string; links: [string, string][] }) {
  return (
    <div>
      <h2 className="text-xs font-semibold uppercase tracking-wide text-ink-subtle">{title}</h2>
      <ul className="mt-3 space-y-2">
        {links.map(([to, label]) => (
          <li key={to}><Link to={to} className="text-sm text-ink-muted hover:text-ink">{label}</Link></li>
        ))}
      </ul>
    </div>
  );
}

/** A centred section with an optional eyebrow, title and intro. */
export function Section({ id, eyebrow, title, intro, children, className }: { id?: string; eyebrow?: string; title: string; intro?: string; children: ReactNode; className?: string }) {
  return (
    <section id={id} aria-labelledby={id ? `${id}-title` : undefined} className={cn("mx-auto max-w-6xl scroll-mt-20 px-4 py-16", className)}>
      {eyebrow && <p className="text-sm font-semibold text-accent">{eyebrow}</p>}
      <h2 id={id ? `${id}-title` : undefined} className="mt-1 max-w-2xl text-2xl font-semibold tracking-tight sm:text-3xl">{title}</h2>
      {intro && <p className="mt-3 max-w-2xl text-lg text-ink-muted">{intro}</p>}
      <div className="mt-10">{children}</div>
    </section>
  );
}
