/** Small set of consistent UI primitives. */
import type { ButtonHTMLAttributes, ReactNode } from "react";
import { X } from "lucide-react";

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "success";

export function Button({
  variant = "secondary",
  size = "md",
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; size?: "sm" | "md" }) {
  const base =
    "inline-flex items-center justify-center gap-1.5 rounded-[5px] font-medium tracking-[0.01em] transition-[background-color,color,border-color,transform] duration-150 active:translate-y-px disabled:opacity-45 disabled:pointer-events-none";
  const sizes = size === "sm" ? "text-[11px] px-2 py-1" : "text-xs px-3 py-1.5";
  const variants: Record<ButtonVariant, string> = {
    // Teal with ink-dark text: reads as a lit indicator, not a glossy CTA.
    primary:
      "bg-accent-600 text-white hover:bg-accent-700 dark:bg-accent-400 dark:text-neutral-950 dark:hover:bg-accent-300",
    secondary:
      "border border-neutral-300 bg-white hover:border-neutral-400 dark:border-neutral-700 dark:bg-neutral-900 dark:hover:border-neutral-600 text-neutral-800 dark:text-neutral-200",
    ghost:
      "hover:bg-neutral-200/70 dark:hover:bg-neutral-800/70 text-neutral-600 dark:text-neutral-300",
    danger: "bg-red-600 hover:bg-red-500 text-white",
    success: "bg-emerald-600 hover:bg-emerald-500 text-white",
  };
  return (
    <button className={`${base} ${sizes} ${variants[variant]} ${className}`} {...rest}>
      {children}
    </button>
  );
}

export function IconButton({
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      className={`inline-flex h-7 w-7 items-center justify-center rounded-[5px] text-neutral-500 transition-colors hover:bg-neutral-200/70 hover:text-neutral-900 disabled:opacity-40 disabled:pointer-events-none dark:text-neutral-400 dark:hover:bg-neutral-800/70 dark:hover:text-neutral-100 ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Spinner({ className = "" }: { className?: string }) {
  return (
    <span
      className={`inline-block h-3.5 w-3.5 animate-spin rounded-full border-[1.5px] border-current border-t-transparent ${className}`}
    />
  );
}

export function Badge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: "neutral" | "green" | "red" | "amber" | "accent" | "blue";
}) {
  const tones: Record<string, string> = {
    neutral:
      "bg-neutral-200 text-neutral-600 dark:bg-neutral-800 dark:text-neutral-400",
    green: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
    red: "bg-red-500/15 text-red-600 dark:text-red-400",
    amber: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
    accent: "bg-accent-500/15 text-accent-600 dark:text-accent-400",
    blue: "bg-sky-500/15 text-sky-600 dark:text-sky-400",
  };
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-[3px] px-1.5 py-px font-mono text-[9.5px] font-medium uppercase tracking-wider ${tones[tone]}`}
    >
      {children}
    </span>
  );
}

export function Modal({
  open,
  title,
  onClose,
  children,
  wide = false,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex animate-fade items-center justify-center bg-neutral-950/60 p-4 backdrop-blur-[2px]">
      <div
        className={`ticks max-h-[85vh] w-full animate-rise ${wide ? "max-w-3xl" : "max-w-lg"} flex flex-col overflow-hidden rounded-md border border-neutral-300 bg-white shadow-[0_24px_60px_-20px_rgba(0,0,0,0.6)] dark:border-neutral-700 dark:bg-neutral-900`}
      >
        <div className="flex items-center justify-between border-b border-neutral-200 dark:border-neutral-800 px-4 py-3">
          <h2 className="font-display text-[15px] font-semibold">{title}</h2>
          <IconButton onClick={onClose} aria-label="Close">
            <X size={14} />
          </IconButton>
        </div>
        <div className="overflow-y-auto px-4 py-4">{children}</div>
      </div>
    </div>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label className="mb-3 block">
      <span className="eyebrow mb-1.5 block">
        {label}
      </span>
      {children}
      {hint && <span className="mt-1 block text-[11px] text-neutral-500">{hint}</span>}
    </label>
  );
}

export const inputClass =
  "w-full rounded-[5px] border border-neutral-300 bg-white px-2.5 py-1.5 text-xs text-neutral-900 placeholder-neutral-400 transition-colors focus:border-accent-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950 dark:text-neutral-200";

export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon?: ReactNode;
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="dot-grid flex h-full animate-fade flex-col items-center justify-center gap-2 p-8 text-center">
      {icon && (
        <div className="ticks mb-1 flex h-11 w-11 items-center justify-center border border-neutral-300 bg-neutral-100 text-neutral-500 dark:border-neutral-800 dark:bg-neutral-925">
          {icon}
        </div>
      )}
      <div className="font-display text-[15px] font-semibold text-neutral-700 dark:text-neutral-200">{title}</div>
      {hint && <div className="max-w-sm text-xs text-neutral-500">{hint}</div>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
}) {
  return (
    <button
      type="button"
      onClick={() => onChange(!checked)}
      className="inline-flex items-center gap-2"
    >
      <span
        className={`relative h-4.5 w-8 rounded-full transition-colors ${checked ? "bg-accent-500" : "bg-neutral-300 dark:bg-neutral-700"}`}
        style={{ height: 18, width: 32 }}
      >
        <span
          className="absolute top-0.5 h-3.5 w-3.5 rounded-full bg-white transition-all"
          style={{ left: checked ? 16 : 2 }}
        />
      </span>
      {label && <span className="text-xs">{label}</span>}
    </button>
  );
}
