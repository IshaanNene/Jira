import { Link } from "react-router-dom";

import { IssueTypeIcon, PriorityIcon, StatusLozenge } from "@/modules/issues";
import { UserChip } from "@/modules/organizations";
import type { BoardCard } from "@/services/api";
import { cn } from "@/utils/cn";

import { formatPoints } from "../model";

export function PointsBadge({ points, className }: { points: number | null | undefined; className?: string }) {
  if (points === null || points === undefined) return null;
  return (
    <span className={cn("inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-line px-1.5 text-xs font-medium text-ink-muted", className)} title="Story points">
      {formatPoints(points)}
    </span>
  );
}

function EpicLabel({ card }: { card: BoardCard }) {
  if (!card.parent_summary) return null;
  return (
    <span className="inline-block max-w-[14rem] truncate rounded-sm bg-[#8F4FD9]/15 px-1.5 text-2xs font-bold uppercase leading-5 tracking-wide text-[#7A3FC0] dark:text-[#B992F0]" title={`${card.parent_key}: ${card.parent_summary}`}>
      {card.parent_summary}
    </span>
  );
}

const issuePath = (card: BoardCard) => `/projects/${card.key.slice(0, card.key.lastIndexOf("-"))}/issues/${card.key}`;

/** A card on the board. */
export function CardTile({ card, overlay = false }: { card: BoardCard; overlay?: boolean }) {
  const done = card.status_category === "done";
  return (
    <article className={cn("rounded border border-line bg-surface p-2.5 shadow-xs", overlay ? "rotate-2 shadow-overlay" : "hover:bg-raised")}>
      <p className="line-clamp-3 text-sm text-ink">{card.summary}</p>
      {card.parent_summary && <div className="mt-1.5"><EpicLabel card={card} /></div>}
      <div className="mt-2 flex items-center gap-2">
        <IssueTypeIcon type={card.type} />
        <Link to={issuePath(card)} className={cn("text-xs font-medium text-ink-muted hover:text-accent hover:underline", done && "line-through")}>
          {card.key}
        </Link>
        <span className="ml-auto flex items-center gap-1.5">
          <PriorityIcon priority={card.priority} />
          <PointsBadge points={card.story_points} />
          <UserChip userId={card.assignee_id} showName={false} />
        </span>
      </div>
    </article>
  );
}

/** One line in the backlog or a sprint. */
export function CardRow({ card, statusName, menu, overlay = false }: { card: BoardCard; statusName: string; menu?: React.ReactNode; overlay?: boolean }) {
  const done = card.status_category === "done";
  return (
    <div className={cn("flex h-10 items-center gap-3 border-b border-line bg-surface px-3 text-sm", overlay ? "rounded border shadow-overlay" : "hover:bg-sunken")}>
      <IssueTypeIcon type={card.type} />
      <Link to={issuePath(card)} className={cn("w-20 shrink-0 font-medium text-ink-muted hover:text-accent hover:underline", done && "line-through")}>
        {card.key}
      </Link>
      <span className="min-w-0 flex-1 truncate text-ink">{card.summary}</span>
      <span className="hidden lg:block"><EpicLabel card={card} /></span>
      <StatusLozenge name={statusName} category={card.status_category} className="hidden max-w-[9rem] sm:inline-flex" />
      <span className="flex w-8 justify-center"><PointsBadge points={card.story_points} /></span>
      <PriorityIcon priority={card.priority} />
      <UserChip userId={card.assignee_id} showName={false} />
      {menu}
    </div>
  );
}
