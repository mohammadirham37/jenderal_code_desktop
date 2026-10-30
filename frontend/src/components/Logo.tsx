/** Brand mark: a rank insignia (star over two chevrons) — "Jenderal". */
export function Logo({ size = 28, className = "" }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 28 28"
      fill="none"
      role="img"
      aria-label="JenderalCode"
      className={className}
    >
      <rect x="0.75" y="0.75" width="26.5" height="26.5" rx="4" className="fill-neutral-950 stroke-accent-500/70" strokeWidth="1.5" />
      <path
        d="M14 4.6l1.32 2.68 2.96.43-2.14 2.08.5 2.94L14 11.34l-2.64 1.39.5-2.94-2.14-2.08 2.96-.43L14 4.6z"
        className="fill-accent-400"
      />
      <path d="M7.5 15.2L14 19l6.5-3.8" className="stroke-accent-400" strokeWidth="2" strokeLinecap="square" strokeLinejoin="miter" />
      <path d="M7.5 19.6L14 23.4l6.5-3.8" className="stroke-accent-400/55" strokeWidth="2" strokeLinecap="square" strokeLinejoin="miter" />
    </svg>
  );
}

/** Wordmark: display-face name with a mono `/code` suffix. */
export function Wordmark({ className = "" }: { className?: string }) {
  return (
    <span className={`inline-flex items-baseline gap-0.5 ${className}`}>
      <span className="font-display font-semibold">Jenderal</span>
      <span className="font-mono text-[0.78em] font-medium text-accent-600 dark:text-accent-400">/code</span>
    </span>
  );
}
