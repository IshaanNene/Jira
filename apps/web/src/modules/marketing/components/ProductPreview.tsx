import { cn } from "@/utils/cn";

const COLUMNS: { name: string; cards: { key: string; title: string; type: "story" | "bug" | "task"; points?: number; who: string }[] }[] = [
  {
    name: "To do",
    cards: [
      { key: "PAY-42", title: "Retry failed card payments", type: "story", points: 5, who: "AL" },
      { key: "PAY-47", title: "Receipt email shows wrong currency", type: "bug", who: "GH" },
    ],
  },
  {
    name: "In progress",
    cards: [
      { key: "PAY-38", title: "Refunds from the order page", type: "story", points: 8, who: "KJ" },
      { key: "PAY-45", title: "Rotate webhook signing keys", type: "task", points: 2, who: "AL" },
    ],
  },
  {
    name: "In review",
    cards: [{ key: "PAY-31", title: "Checkout fails on Safari 17", type: "bug", points: 3, who: "MH" }],
  },
  {
    name: "Done",
    cards: [
      { key: "PAY-29", title: "Saved cards for returning customers", type: "story", points: 5, who: "GH" },
      { key: "PAY-33", title: "Upgrade payment SDK", type: "task", points: 1, who: "KJ" },
    ],
  },
];

const TYPE_COLOR = { story: "bg-success", bug: "bg-danger", task: "bg-accent" } as const;

/** An illustration of a sprint board, drawn in HTML so it follows the theme. Decorative. */
export function ProductPreview({ className }: { className?: string }) {
  return (
    <div
      role="img"
      aria-label="A sprint board with issues in To do, In progress, In review and Done columns"
      className={cn("overflow-hidden rounded-lg border border-line bg-surface shadow-overlay", className)}
    >
      <div className="flex items-center gap-1.5 border-b border-line bg-sunken px-3 py-2" aria-hidden>
        <span className="h-2.5 w-2.5 rounded-full bg-line-strong" />
        <span className="h-2.5 w-2.5 rounded-full bg-line-strong" />
        <span className="h-2.5 w-2.5 rounded-full bg-line-strong" />
        <span className="ml-3 text-xs text-ink-subtle">Payments · Sprint 14</span>
      </div>
      <div className="grid grid-cols-2 gap-3 p-3 md:grid-cols-4" aria-hidden>
        {COLUMNS.map((col) => (
          <div key={col.name} className="rounded bg-sunken p-2">
            <div className="mb-2 flex items-center justify-between px-1 text-2xs font-semibold uppercase tracking-wide text-ink-subtle">
              <span>{col.name}</span>
              <span>{col.cards.length}</span>
            </div>
            <div className="space-y-2">
              {col.cards.map((c) => (
                <div key={c.key} className="rounded border border-line bg-surface p-2 shadow-xs">
                  <p className={cn("text-xs leading-snug text-ink", col.name === "Done" && "text-ink-muted line-through decoration-ink-subtle/50")}>{c.title}</p>
                  <div className="mt-2 flex items-center gap-1.5">
                    <span className={cn("h-3 w-3 rounded-sm", TYPE_COLOR[c.type])} />
                    <span className="text-2xs font-medium text-ink-subtle">{c.key}</span>
                    {c.points !== undefined && <span className="ml-auto rounded-full bg-sunken px-1.5 text-2xs text-ink-muted">{c.points}</span>}
                    <span className={cn("flex h-5 w-5 items-center justify-center rounded-full bg-accent-soft text-[0.5625rem] font-semibold text-accent", c.points === undefined && "ml-auto")}>{c.who}</span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
