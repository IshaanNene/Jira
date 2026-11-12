import { cn } from "@/utils/cn";

const COLORS = ["#0C66E4", "#1F845A", "#8F4FD9", "#E2483D", "#C25100", "#1D7F8C", "#943D73"];

/** Square project tile coloured from the key, so a project looks the same everywhere. */
export function ProjectAvatar({ projectKey, size = "md", className }: { projectKey: string; size?: "sm" | "md" | "lg"; className?: string }) {
  let hash = 0;
  for (const ch of projectKey) hash = (hash * 31 + ch.charCodeAt(0)) % COLORS.length;
  const sizes = { sm: "h-5 w-5 text-[9px]", md: "h-6 w-6 text-2xs", lg: "h-8 w-8 text-xs" };
  return (
    <span aria-hidden className={cn("inline-flex shrink-0 items-center justify-center rounded-sm font-bold text-white", sizes[size], className)} style={{ backgroundColor: COLORS[hash] }}>
      {projectKey.slice(0, 2)}
    </span>
  );
}
