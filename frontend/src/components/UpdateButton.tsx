/** Compact "check for updates" control for page headers (Welcome, the
 *  workspace top bar). A manual check that finds a release opens the
 *  update dialog; a known update shows as a highlighted button. */
import { ArrowUpCircle, Check, RotateCw } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { useUpdater } from "@/stores/updater";
import { Spinner } from "@/components/ui";

async function checkAndOpen() {
  const u = useUpdater.getState();
  await u.checkNow();
  if (useUpdater.getState().status === "available") useUpdater.getState().setDialogOpen(true);
}

export function UpdateButton({ showVersion = false }: { showVersion?: boolean }) {
  const u = useUpdater(
    useShallow((s) => ({ status: s.status, current: s.current, version: s.update?.version ?? null, error: s.error })),
  );
  const base =
    "flex items-center gap-1.5 rounded-[5px] border px-2 py-1 font-mono text-[10px] tracking-wider uppercase transition-colors disabled:opacity-60";

  if (u.version && u.status !== "current" && u.status !== "checking") {
    return (
      <button
        title={`Update to v${u.version}`}
        onClick={() => useUpdater.getState().setDialogOpen(true)}
        className={`${base} border-accent-500/60 bg-accent-500/10 text-accent-700 hover:bg-accent-500/20 dark:text-accent-300`}
      >
        <ArrowUpCircle size={12} /> Update v{u.version}
      </button>
    );
  }

  const busy = u.status === "checking" || u.status === "downloading" || u.status === "restarting";
  const label =
    u.status === "checking" ? "Checking…" : u.status === "current" ? "Up to date" : u.status === "error" ? "Retry check" : "Check updates";
  const title =
    u.status === "error" && u.error
      ? `Could not check for updates: ${u.error}`
      : `JenderalCode${u.current ? ` v${u.current}` : ""} — check for updates`;

  return (
    <button
      title={title}
      disabled={busy}
      onClick={() => void checkAndOpen()}
      className={`${base} ${
        u.status === "error"
          ? "border-red-500/50 text-red-500 hover:bg-red-500/10"
          : "border-neutral-300 text-neutral-500 hover:border-accent-500 hover:text-accent-600 dark:border-neutral-700 dark:hover:text-accent-400"
      }`}
    >
      {u.status === "checking" ? (
        <Spinner className="h-3 w-3" />
      ) : u.status === "current" ? (
        <Check size={12} />
      ) : (
        <RotateCw size={12} />
      )}
      {label}
      {showVersion && u.current && <span className="text-neutral-400 normal-case">v{u.current}</span>}
    </button>
  );
}
