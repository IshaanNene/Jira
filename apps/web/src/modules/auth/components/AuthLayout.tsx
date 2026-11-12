import type { ReactNode } from "react";

import { PRODUCT_NAME } from "@/utils/config";

/** Centered single-column frame for sign-in, sign-up and invitation screens. */
export function AuthLayout({ title, subtitle, children, footer }: { title: string; subtitle?: ReactNode; children: ReactNode; footer?: ReactNode }) {
  return (
    <main className="flex min-h-full flex-col items-center justify-center bg-canvas px-4 py-10">
      <div className="w-full max-w-[22.5rem]">
        <div className="mb-6 flex items-center gap-2">
          <img src="/mark.svg" alt="" className="h-7 w-7" />
          <span className="text-lg font-semibold tracking-tight text-ink">{PRODUCT_NAME}</span>
        </div>
        <h1 className="text-2xl font-semibold tracking-tight text-ink">{title}</h1>
        {subtitle && <p className="mt-1.5 text-base text-ink-muted">{subtitle}</p>}
        <div className="mt-6">{children}</div>
        {footer && <p className="mt-6 text-sm text-ink-muted">{footer}</p>}
      </div>
    </main>
  );
}
