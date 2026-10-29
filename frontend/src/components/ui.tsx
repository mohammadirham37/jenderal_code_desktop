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
    "inline-flex items-center justify-center gap-1.5 rounded-md font-medium transition-colors disabled:opacity-50 disabled:pointer-events-none focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500/60";
  const sizes = size === "sm" ? "text-[11px] px-2 py-1" : "text-xs px-3 py-1.5";
  const variants: Record<ButtonVariant, string> = {
    primary: "bg-indigo-600 hover:bg-indigo-500 text-white shadow-sm",
    secondary:
      "bg-neutral-200 hover:bg-neutral-300 dark:bg-neutral-800 dark:hover:bg-neutral-700 text-neutral-800 dark:text-neutral-200",
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
      className={`inline-flex h-7 w-7 items-center justify-center rounded-md text-neutral-500 hover:bg-neutral-200/70 dark:text-neutral-400 dark:hover:bg-neutral-800/70 transition-colors focus:outline-none ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Spinner({ className = "" }: { className?: string }) {
  return (
    <span
      className={`inline-block h-3.5 w-3.5 animate-spin rounded-full border-2 border-current border-t-transparent ${className}`}
    />
  );
}

export function Badge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: "neutral" | "green" | "red" | "amber" | "indigo" | "blue";
}) {
  const tones: Record<string, string> = {
    neutral:
      "bg-neutral-200 text-neutral-600 dark:bg-neutral-800 dark:text-neutral-400",
    green: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
    red: "bg-red-500/15 text-red-600 dark:text-red-400",
    amber: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
    indigo: "bg-indigo-500/15 text-indigo-600 dark:text-indigo-400",
    blue: "bg-sky-500/15 text-sky-600 dark:text-sky-400",
  };
  return (
    <span
      className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium ${tones[tone]}`}
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
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div
        className={`max-h-[85vh] w-full ${wide ? "max-w-3xl" : "max-w-lg"} overflow-hidden rounded-xl border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-[#15171d] shadow-2xl flex flex-col`}
      >
        <div className="flex items-center justify-between border-b border-neutral-200 dark:border-neutral-800 px-4 py-3">
          <h2 className="text-sm font-semibold">{title}</h2>
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
      <span className="mb-1 block text-[11px] font-medium uppercase tracking-wide text-neutral-500">
        {label}
      </span>
      {children}
      {hint && <span className="mt-1 block text-[11px] text-neutral-500">{hint}</span>}
    </label>
  );
}

export const inputClass =
  "w-full rounded-md border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-[#0d0f14] px-2.5 py-1.5 text-xs text-neutral-900 dark:text-neutral-200 placeholder-neutral-400 focus:outline-none focus:ring-2 focus:ring-indigo-500/50";

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
    <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
      {icon && <div className="text-neutral-400 dark:text-neutral-600">{icon}</div>}
      <div className="text-sm font-medium text-neutral-600 dark:text-neutral-300">{title}</div>
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
        className={`relative h-4.5 w-8 rounded-full transition-colors ${checked ? "bg-indigo-600" : "bg-neutral-300 dark:bg-neutral-700"}`}
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
