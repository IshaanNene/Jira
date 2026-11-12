import {
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  pointerWithin,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type CollisionDetection,
  type DragEndEvent,
} from "@dnd-kit/core";
import type { ReactNode } from "react";

import type { BoardCard } from "@/services/api";
import { cn } from "@/utils/cn";

/** Where something was dropped: a list (column, sprint or backlog) and optionally the card it landed on. */
export interface DropTarget {
  list: string;
  beforeId?: string;
}

/** Prefer the card under the pointer over the list that contains it. Keyboard drags have no pointer. */
export const collision: CollisionDetection = (args) => {
  const hits = pointerWithin(args);
  const pool = hits.length > 0 ? hits : closestCenter(args);
  const card = pool.find((hit) => String(hit.id).startsWith("card:"));
  return card ? [card] : pool.slice(0, 1);
};

/** A small movement threshold keeps clicks on cards (links, menus) working. */
export function useCardSensors() {
  return useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }), useSensor(KeyboardSensor));
}

/** The dragged card and where it was dropped, or null when the drop means nothing. */
export function readDrop(event: DragEndEvent): { card: BoardCard; target: DropTarget } | null {
  const card = event.active.data.current?.card as BoardCard | undefined;
  const target = event.over?.data.current as DropTarget | undefined;
  if (!card || !target || target.beforeId === card.id) return null;
  return { card, target };
}

/** A card that can be picked up and that other cards can be dropped onto (to go before it). */
export function DraggableCard({ card, list, disabled, children }: { card: BoardCard; list: string; disabled: boolean; children: ReactNode }) {
  const drag = useDraggable({ id: `drag:${card.id}`, data: { card }, disabled });
  const drop = useDroppable({ id: `card:${card.id}`, data: { list, beforeId: card.id } satisfies DropTarget });
  return (
    <div
      ref={(node) => {
        drag.setNodeRef(node);
        drop.setNodeRef(node);
      }}
      {...drag.attributes}
      {...drag.listeners}
      aria-roledescription={disabled ? undefined : "draggable issue"}
      className={cn("relative touch-none outline-none", !disabled && "cursor-grab", drag.isDragging && "opacity-40")}
    >
      {drop.isOver && !drag.isDragging && <span className="pointer-events-none absolute inset-x-0 -top-[3px] z-10 h-0.5 rounded bg-accent" aria-hidden />}
      {children}
    </div>
  );
}

/** A column, sprint or backlog: dropping on its empty space appends to it. */
export function DropList({ list, className, children }: { list: string; className?: string; children: ReactNode }) {
  const { setNodeRef, isOver } = useDroppable({ id: `list:${list}`, data: { list } satisfies DropTarget });
  return (
    <div ref={setNodeRef} className={cn(className, isOver && "ring-2 ring-inset ring-accent/50")}>
      {children}
    </div>
  );
}
