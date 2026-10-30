/** "Usage" button in the chat column's tab strip: subscription limits of the
 *  external agent CLIs (Claude Code, Codex, ...). Loads when opened. */
import { useEffect, useRef, useState } from "react";
import { Gauge, RotateCw } from "lucide-react";
import { api, type AgentUsageReport, type AgentUsageStatus, type UsageWindow } from "@/lib/ipc";
import { formatRelative, formatUntil } from "@/lib/format";
import { IconButton } from "@/components/ui";

/** Reopening within this window shows the cached reports. */
const STALE_MS = 60_000;

let cache: { at: number; reports: AgentUsageReport[] } | null = null;

const STATUS_LABEL: Record<AgentUsageStatus, string | null> = {
  ok: null,
  not_installed: "Not installed",
  not_signed_in: "Not signed in",
  expired: "Login expired",
  unavailable: "No quota data",
  error: "Error",
};

export function AgentUsageButton() {
  const [open, setOpen] = useState(false);
  const [reports, setReports] = useState<AgentUsageReport[] | null>(cache?.reports ?? null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const r = await api.agentUsage();
      cache = { at: Date.now(), reports: r };
      setReports(r);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    if (!cache || Date.now() - cache.at > STALE_MS) void load();
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative flex items-center">
      <IconButton title="Agent usage" className="h-6 w-6" onClick={() => setOpen((o) => !o)}>
        <Gauge size={13} />
      </IconButton>
      {open && (
        <div className="animate-rise absolute left-0 top-full z-30 mt-1 w-80 rounded-[6px] border border-neutral-200 bg-white shadow-lg dark:border-neutral-800 dark:bg-neutral-900">
          <div className="flex items-center justify-between border-b border-neutral-200 px-3 py-2 dark:border-neutral-800">
            <span className="eyebrow">Agent usage</span>
            <IconButton title="Refresh" className="h-6 w-6" disabled={loading} onClick={() => void load()}>
              <RotateCw size={12} className={loading ? "animate-spin" : ""} />
            </IconButton>
          </div>
          <div className="max-h-[70vh] overflow-y-auto p-1">
            {error && <div className="px-2 py-3 text-xs text-red-500">{error}</div>}
            {!reports && !error && <div className="px-2 py-3 text-xs text-neutral-500">Loading…</div>}
            {reports?.map((r) => <ReportCard key={r.id} report={r} />)}
          </div>
        </div>
      )}
    </div>
  );
}

function ReportCard({ report }: { report: AgentUsageReport }) {
  const status = STATUS_LABEL[report.status];
  const dim = report.status === "not_installed";
  return (
    <div className={`rounded-[4px] px-2 py-2 ${dim ? "opacity-60" : ""}`}>
      <div className="flex items-baseline gap-2">
        <span className="text-xs font-medium text-neutral-900 dark:text-neutral-100">{report.name}</span>
        {report.plan && <span className="font-mono text-[10px] text-accent-600 dark:text-accent-400">{report.plan}</span>}
        {status && (
          <span
            className={`ml-auto font-mono text-[10px] ${
              report.status === "error" || report.status === "expired" ? "text-red-500" : "text-neutral-500"
            }`}
          >
            {status}
          </span>
        )}
      </div>
      {report.account && <div className="mt-0.5 font-mono text-[10px] text-neutral-500">{report.account}</div>}
      {report.windows.map((w) => (
        <WindowRow key={w.label} window={w} />
      ))}
      {(report.note || report.updated_at) && (
        <div className="mt-1.5 text-[10.5px] leading-snug text-neutral-500">
          {report.note}
          {report.updated_at && <span className="font-mono"> · {formatRelative(report.updated_at)}</span>}
        </div>
      )}
    </div>
  );
}

function WindowRow({ window: w }: { window: UsageWindow }) {
  const pct = w.used_percent;
  const bar = pct === null ? null : Math.min(100, Math.max(0, pct));
  const tone = bar !== null && bar >= 90 ? "bg-red-500" : "bg-accent-500";
  return (
    <div className="mt-2">
      <div className="flex items-baseline justify-between gap-2 text-[11px]">
        <span className="min-w-0 truncate text-neutral-600 dark:text-neutral-300">{w.label}</span>
        {pct !== null && <span className="shrink-0 font-mono text-neutral-800 dark:text-neutral-200">{Math.round(pct)}%</span>}
      </div>
      {bar !== null && (
        <div className="mt-1 h-1 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-800">
          <div className={`h-full ${tone}`} style={{ width: `${bar}%` }} />
        </div>
      )}
      {(w.resets_at || w.detail) && (
        <div className="mt-0.5 font-mono text-[10px] text-neutral-500">
          {w.resets_at ? `resets ${formatUntil(w.resets_at)}` : null}
          {w.resets_at && w.detail ? " · " : null}
          {w.detail}
        </div>
      )}
    </div>
  );
}
