import { DndContext, KeyboardSensor, PointerSensor, useDraggable, useSensor, useSensors, type DragEndEvent } from "@dnd-kit/core";
import { Flag, Link2 } from "lucide-react";

import type { WorkflowDefinition, WorkflowStatus } from "@/services/api";
import { cn } from "@/utils/cn";

import { edgePoints, NODE_HEIGHT, NODE_WIDTH } from "../model";

export type Selection = { kind: "status"; key: string } | { kind: "transition"; id: string } | null;

const CATEGORY_STYLE: Record<string, string> = {
  todo: "border-line-strong bg-sunken text-ink",
  in_progress: "border-accent/50 bg-accent-soft text-accent",
  done: "border-success/50 bg-success-soft text-success",
};

interface CanvasProps {
  definition: WorkflowDefinition;
  selection: Selection;
  onSelect: (selection: Selection) => void;
  /** Status the next clicked status will be connected from, if connecting. */
  connectingFrom: string | null;
  onStartConnect: (key: string) => void;
  onConnect: (from: string, to: string) => void;
  onMove: (key: string, dx: number, dy: number) => void;
  readOnly: boolean;
}

/** The workflow diagram: draggable statuses, arrows for transitions. */
export function Canvas({ definition, selection, onSelect, connectingFrom, onStartConnect, onConnect, onMove, readOnly }: CanvasProps) {
  // A small drag threshold keeps a click a click (select) rather than a drag.
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }), useSensor(KeyboardSensor));
  const byKey = new Map(definition.statuses.map((s) => [s.key, s]));
  const width = Math.max(960, ...definition.statuses.map((s) => (s.x ?? 0) + NODE_WIDTH + 120));
  const height = Math.max(480, ...definition.statuses.map((s) => (s.y ?? 0) + NODE_HEIGHT + 120));
  const onDragEnd = (event: DragEndEvent) => onMove(String(event.active.id), event.delta.x, event.delta.y);

  return (
    <div className="h-full overflow-auto bg-sunken/50" onClick={() => onSelect(null)}>
      <DndContext sensors={sensors} onDragEnd={onDragEnd}>
        <div
          className="relative"
          style={{ width, height, backgroundImage: "radial-gradient(hsl(var(--line-strong)) 1px, transparent 1px)", backgroundSize: "20px 20px" }}
        >
          <svg className="absolute inset-0" width={width} height={height} aria-hidden>
            <defs>
              <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                <path d="M 0 0 L 10 5 L 0 10 z" fill="hsl(var(--ink-subtle))" />
              </marker>
              <marker id="arrow-selected" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                <path d="M 0 0 L 10 5 L 0 10 z" fill="hsl(var(--accent))" />
              </marker>
            </defs>
            {definition.transitions.flatMap((t) => {
              const to = byKey.get(t.to);
              const selected = selection?.kind === "transition" && selection.id === t.id;
              return (t.from ?? []).map((fromKey) => {
                const from = byKey.get(fromKey);
                if (!from || !to || from.key === to.key) return null;
                const { start, end } = edgePoints(from, to);
                return (
                  <line
                    key={`${t.id}:${fromKey}`}
                    x1={start.x} y1={start.y} x2={end.x} y2={end.y}
                    stroke={selected ? "hsl(var(--accent))" : "hsl(var(--ink-subtle))"}
                    strokeWidth={selected ? 2 : 1.25}
                    markerEnd={selected ? "url(#arrow-selected)" : "url(#arrow)"}
                  />
                );
              });
            })}
          </svg>

          {/* Transition labels are real buttons so they can be selected with the keyboard. */}
          {definition.transitions.flatMap((t) => {
            const to = byKey.get(t.to);
            if (!to) return [];
            const selected = selection?.kind === "transition" && selection.id === t.id;
            const chip = (key: string, x: number, y: number, text: string) => (
              <button
                key={key}
                type="button"
                onClick={(e) => { e.stopPropagation(); onSelect({ kind: "transition", id: t.id }); }}
                style={{ left: x, top: y }}
                className={cn(
                  "absolute -translate-x-1/2 -translate-y-1/2 whitespace-nowrap rounded-full border px-2 py-0.5 text-2xs font-medium shadow-xs",
                  selected ? "border-accent bg-accent text-accent-ink" : "border-line bg-surface text-ink-muted hover:border-line-strong hover:text-ink",
                )}
              >
                {text}
              </button>
            );
            if ((t.from ?? []).length === 0) {
              // "From any status": shown as a tag on the target rather than a web of arrows.
              return [chip(t.id, (to.x ?? 0) + NODE_WIDTH / 2, (to.y ?? 0) - 14, `Any → ${t.name}`)];
            }
            return (t.from ?? []).flatMap((fromKey) => {
              const from = byKey.get(fromKey);
              if (!from || from.key === to.key) return [];
              const { label } = edgePoints(from, to);
              return [chip(`${t.id}:${fromKey}`, label.x, label.y, t.name)];
            });
          })}

          {definition.statuses.map((status) => (
            <StatusNode
              key={status.key}
              status={status}
              initial={definition.initial === status.key}
              selected={selection?.kind === "status" && selection.key === status.key}
              connecting={connectingFrom}
              readOnly={readOnly}
              onSelect={() => (connectingFrom && connectingFrom !== status.key ? onConnect(connectingFrom, status.key) : onSelect({ kind: "status", key: status.key }))}
              onStartConnect={() => onStartConnect(status.key)}
            />
          ))}
        </div>
      </DndContext>
    </div>
  );
}

interface NodeProps {
  status: WorkflowStatus;
  initial: boolean;
  selected: boolean;
  connecting: string | null;
  readOnly: boolean;
  onSelect: () => void;
  onStartConnect: () => void;
}

function StatusNode({ status, initial, selected, connecting, readOnly, onSelect, onStartConnect }: NodeProps) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({ id: status.key, disabled: readOnly });
  const isSource = connecting === status.key;
  return (
    <div
      ref={setNodeRef}
      style={{
        left: status.x ?? 0,
        top: status.y ?? 0,
        width: NODE_WIDTH,
        height: NODE_HEIGHT,
        transform: transform ? `translate(${transform.x}px, ${transform.y}px)` : undefined,
      }}
      className={cn("group absolute", isDragging && "z-10 opacity-80")}
    >
      <button
        type="button"
        {...attributes}
        {...listeners}
        onClick={(e) => { e.stopPropagation(); onSelect(); }}
        aria-pressed={selected}
        aria-label={`${status.name}${initial ? ", initial status" : ""}`}
        className={cn(
          "flex h-full w-full items-center justify-center gap-1.5 rounded border-2 px-2 text-sm font-semibold shadow-xs",
          CATEGORY_STYLE[status.category] ?? CATEGORY_STYLE.todo,
          selected && "ring-2 ring-accent ring-offset-2 ring-offset-surface",
          isSource && "ring-2 ring-warning ring-offset-2 ring-offset-surface",
          connecting && !isSource && "cursor-crosshair",
          !readOnly && !connecting && "cursor-grab active:cursor-grabbing",
        )}
      >
        {initial && <Flag className="h-3.5 w-3.5 shrink-0" aria-hidden />}
        <span className="truncate">{status.name}</span>
      </button>
      {!readOnly && !connecting && (
        <button
          type="button"
          title={`Add a transition from ${status.name}`}
          aria-label={`Add a transition from ${status.name}`}
          onClick={(e) => { e.stopPropagation(); onStartConnect(); }}
          className="absolute -right-3 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-full border border-line-strong bg-surface text-ink-muted opacity-0 shadow-sm transition-opacity hover:text-accent focus-visible:opacity-100 group-hover:opacity-100"
        >
          <Link2 className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}
