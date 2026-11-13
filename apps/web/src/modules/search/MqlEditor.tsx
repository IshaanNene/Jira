import { CircleAlert, CornerDownLeft } from "lucide-react";
import { useId, useMemo, useRef, useState, type KeyboardEvent } from "react";

import { Button } from "@/components/ui";
import { can, useMembers } from "@/modules/organizations";
import { useProjects } from "@/modules/projects";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";

import { useDebounced } from "./api";
import { applySuggestion, suggest, type Suggestion } from "./mql";

export interface QueryProblem {
  /** The query the problem was found in. */
  query: string;
  message: string;
  span?: [number, number];
}

/**
 * A query box with suggestions for what can come next. Enter runs the query (or picks the
 * highlighted suggestion); Shift+Enter starts a new line.
 */
export function MqlEditor({
  value,
  onChange,
  onRun,
  problem,
}: {
  value: string;
  onChange: (value: string) => void;
  onRun: (value: string) => void;
  problem?: QueryProblem;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const listId = useId();
  const [caret, setCaret] = useState(value.length);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  // Enter takes a suggestion only after the arrow keys were used to choose one.
  const [navigated, setNavigated] = useState(false);

  const role = useSession((s) => s.organization?.role);
  const projects = useProjects();
  const base = useMemo(() => suggest(value, caret, { projectKeys: [], people: [] }), [value, caret]);
  const wantsPeople = base.valuesFor === "assignee" || base.valuesFor === "reporter";
  const peopleSearch = useDebounced(wantsPeople ? base.partial : "", 200);
  const members = useMembers(peopleSearch, open && wantsPeople && can(role, "viewMembers"));

  const suggestions = useMemo(
    () =>
      suggest(value, caret, {
        projectKeys: (projects.data ?? []).map((p) => p.key),
        people: (members.data?.pages[0]?.items ?? []).slice(0, 6).map((m) => ({ name: m.display_name, email: m.email })),
      }),
    [value, caret, projects.data, members.data],
  );
  const items = open ? suggestions.items : [];
  const highlighted = items[Math.min(active, items.length - 1)];

  const syncCaret = () => setCaret(ref.current?.selectionStart ?? value.length);

  const pick = (item: Suggestion) => {
    const next = applySuggestion(value, suggestions, item);
    onChange(next.text);
    setCaret(next.caret);
    setActive(0);
    setNavigated(false);
    requestAnimationFrame(() => {
      ref.current?.focus();
      ref.current?.setSelectionRange(next.caret, next.caret);
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (items.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      setActive((a) => (navigated ? (a + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length : e.key === "ArrowDown" ? 0 : items.length - 1));
      setNavigated(true);
      return;
    }
    if (highlighted && e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      pick(highlighted);
      return;
    }
    if (e.key === "Escape" && open) {
      e.preventDefault();
      setOpen(false);
      return;
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      // Only a suggestion the person moved to is taken; a plain Enter runs the query.
      if (highlighted && navigated) {
        pick(highlighted);
      } else {
        setOpen(false);
        onRun(value);
      }
    }
  };

  const showProblem = problem && problem.query === value;

  return (
    <div>
      <div className="flex items-start gap-2">
        <div className="relative flex-1">
          <textarea
            ref={ref}
            role="combobox"
            aria-label="MQL query"
            aria-expanded={items.length > 0}
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={highlighted && navigated ? `${listId}-${items.indexOf(highlighted)}` : undefined}
            aria-invalid={showProblem || undefined}
            aria-describedby={showProblem ? `${listId}-problem` : undefined}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            rows={2}
            maxLength={2000}
            value={value}
            placeholder="project = PAY AND assignee = currentUser() ORDER BY updated DESC"
            onChange={(e) => {
              onChange(e.target.value);
              setCaret(e.target.selectionStart);
              setActive(0);
              setNavigated(false);
              setOpen(true);
            }}
            onKeyDown={onKeyDown}
            onKeyUp={(e) => {
              const moved = e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "Home" || e.key === "End";
              if (moved || (e.key.startsWith("Arrow") && !items.length)) syncCaret();
            }}
            onClick={() => {
              syncCaret();
              setOpen(true);
            }}
            onFocus={() => setOpen(true)}
            onBlur={() => setOpen(false)}
            className={cn(
              "block w-full resize-y rounded border bg-surface px-3 py-2 font-mono text-sm text-ink shadow-xs placeholder:text-ink-subtle",
              "focus-visible:border-accent focus-visible:ring-2 focus-visible:ring-accent/25 focus-visible:ring-offset-0",
              showProblem ? "border-danger" : "border-line-strong hover:border-ink-subtle",
            )}
          />
          {items.length > 0 && (
            <ul
              id={listId}
              role="listbox"
              aria-label="Suggestions"
              className="absolute left-0 top-full z-20 mt-1 max-h-72 w-full max-w-md overflow-auto rounded border border-line bg-surface py-1 shadow-overlay"
            >
              {items.map((item, i) => (
                <li
                  key={item.label}
                  id={`${listId}-${i}`}
                  role="option"
                  aria-selected={item === highlighted && navigated}
                  // Keep focus in the editor while choosing.
                  onMouseDown={(e) => {
                    e.preventDefault();
                    pick(item);
                  }}
                  onMouseEnter={() => setActive(i)}
                  className={cn(
                    "flex cursor-pointer items-baseline justify-between gap-4 px-3 py-1.5 text-sm",
                    item === highlighted && navigated ? "bg-accent-soft text-ink" : "text-ink-muted hover:bg-sunken",
                  )}
                >
                  <span className="font-mono text-ink">{item.label}</span>
                  {item.detail && <span className="truncate text-xs text-ink-subtle">{item.detail}</span>}
                </li>
              ))}
            </ul>
          )}
        </div>
        <Button onClick={() => onRun(value)}>
          Search
          <CornerDownLeft className="h-3.5 w-3.5 opacity-70" aria-hidden />
        </Button>
      </div>

      {showProblem && (
        <div id={`${listId}-problem`} role="alert" className="mt-2 rounded border border-danger/40 bg-danger-soft px-3 py-2 text-sm">
          <p className="flex items-center gap-1.5 font-medium text-danger">
            <CircleAlert className="h-4 w-4 shrink-0" aria-hidden />
            {problem.message}
          </p>
          {problem.span && <MarkedQuery query={problem.query} span={problem.span} />}
        </div>
      )}
    </div>
  );
}

/** The query with the part at fault underlined (a caret mark when the span is empty). */
export function MarkedQuery({ query, span }: { query: string; span: [number, number] }) {
  // Spans count characters, not UTF-16 units.
  const chars = Array.from(query);
  const [start, end] = [Math.min(span[0], chars.length), Math.min(span[1], chars.length)];
  return (
    <pre className="mt-1.5 overflow-x-auto whitespace-pre-wrap break-all font-mono text-xs text-ink-muted">
      {chars.slice(0, start).join("")}
      {end > start ? (
        <mark className="bg-transparent text-danger underline decoration-danger decoration-wavy underline-offset-4">
          {chars.slice(start, end).join("")}
        </mark>
      ) : (
        <mark aria-label="here" className="bg-transparent font-bold text-danger">
          ▏
        </mark>
      )}
      {chars.slice(end).join("")}
    </pre>
  );
}
