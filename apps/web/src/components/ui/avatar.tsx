import { cn } from "@/utils/cn";
import { initials } from "@/utils/format";

const sizes = { sm: "h-6 w-6 text-2xs", md: "h-8 w-8 text-xs", lg: "h-10 w-10 text-sm" } as const;

// A stable hue per person so the same name always gets the same colour.
function hueFor(seed: string): number {
  let hash = 0;
  for (let i = 0; i < seed.length; i += 1) hash = (hash * 31 + seed.charCodeAt(i)) % 360;
  return hash;
}

interface AvatarProps {
  name: string;
  /** Stable identifier used for the colour (falls back to the name). */
  seed?: string;
  size?: keyof typeof sizes;
  square?: boolean;
  className?: string;
}

export function Avatar({ name, seed, size = "md", square = false, className }: AvatarProps) {
  const hue = hueFor(seed ?? name);
  return (
    <span
      aria-hidden
      className={cn("inline-flex shrink-0 select-none items-center justify-center font-semibold", square ? "rounded" : "rounded-full", sizes[size], className)}
      style={{ backgroundColor: `hsl(${hue} 42% 88%)`, color: `hsl(${hue} 45% 24%)` }}
    >
      {initials(name)}
    </span>
  );
}
