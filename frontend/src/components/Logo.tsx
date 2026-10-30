/** Brand mark: a "JC" monogram (J set inside an open C) on graphite with
 *  corner ticks. Same artwork as the app icon (`apps/desktop/icons/icon-source.svg`). */
export function Logo({ size = 28, className = "" }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="100 100 824 824"
      fill="none"
      role="img"
      aria-label="JenderalCode"
      className={className}
    >
      <rect x="100" y="100" width="824" height="824" rx="186" className="fill-neutral-950" />
      <rect x="110" y="110" width="804" height="804" rx="176" className="stroke-accent-500/30" strokeWidth="16" />
      <g className="stroke-accent-400" strokeWidth="16" strokeLinecap="square">
        <path d="M222 262V222H262" />
        <path d="M762 222H802V262" />
        <path d="M802 762V802H762" />
        <path d="M262 802H222V762" />
      </g>
      <g transform="translate(24 0)">
        <path d="M682.8 336.3A250 250 0 1 0 682.8 687.7" className="stroke-accent-500" strokeWidth="92" />
        <path d="M506 372H632" className="stroke-accent-200" strokeWidth="64" />
        <path d="M600 372V566A72 72 0 0 1 456 566" className="stroke-accent-200" strokeWidth="64" />
      </g>
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
