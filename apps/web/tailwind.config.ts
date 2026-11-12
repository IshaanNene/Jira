import type { Config } from "tailwindcss";

// Colours are CSS variables (see src/styles/index.css) so light and dark themes share
// one set of utility classes. Components use semantic names, never raw palette values.
const token = (name: string) => `hsl(var(--${name}) / <alpha-value>)`;

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: ["class"],
  theme: {
    extend: {
      colors: {
        canvas: token("canvas"),
        surface: token("surface"),
        raised: token("raised"),
        sunken: token("sunken"),
        line: token("line"),
        "line-strong": token("line-strong"),
        ink: token("ink"),
        "ink-muted": token("ink-muted"),
        "ink-subtle": token("ink-subtle"),
        accent: { DEFAULT: token("accent"), hover: token("accent-hover"), soft: token("accent-soft"), ink: token("accent-ink") },
        danger: { DEFAULT: token("danger"), soft: token("danger-soft") },
        warning: { DEFAULT: token("warning"), soft: token("warning-soft") },
        success: { DEFAULT: token("success"), soft: token("success-soft") },
      },
      fontFamily: {
        sans: ['"Inter Variable"', "Inter", "ui-sans-serif", "system-ui", "sans-serif"],
        mono: ["ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
      },
      fontSize: {
        "2xs": ["0.6875rem", { lineHeight: "1rem" }],
        xs: ["0.75rem", { lineHeight: "1rem" }],
        sm: ["0.8125rem", { lineHeight: "1.25rem" }],
        base: ["0.875rem", { lineHeight: "1.375rem" }],
        lg: ["1rem", { lineHeight: "1.5rem" }],
        xl: ["1.25rem", { lineHeight: "1.75rem" }],
        "2xl": ["1.5rem", { lineHeight: "2rem" }],
      },
      borderRadius: { sm: "3px", DEFAULT: "3px", md: "4px", lg: "6px", xl: "8px" },
      boxShadow: {
        xs: "0 1px 1px hsl(var(--shadow) / 0.04)",
        sm: "0 1px 2px hsl(var(--shadow) / 0.06), 0 1px 1px hsl(var(--shadow) / 0.04)",
        overlay: "0 8px 24px hsl(var(--shadow) / 0.12), 0 2px 6px hsl(var(--shadow) / 0.08)",
      },
      keyframes: {
        "fade-in": { from: { opacity: "0" }, to: { opacity: "1" } },
        "pop-in": { from: { opacity: "0", transform: "translateY(4px) scale(0.98)" }, to: { opacity: "1", transform: "none" } },
        shimmer: { "100%": { transform: "translateX(100%)" } },
      },
      animation: {
        "fade-in": "fade-in 120ms ease-out",
        "pop-in": "pop-in 140ms cubic-bezier(0.2, 0.8, 0.2, 1)",
        shimmer: "shimmer 1.4s infinite",
      },
    },
  },
  plugins: [],
} satisfies Config;
