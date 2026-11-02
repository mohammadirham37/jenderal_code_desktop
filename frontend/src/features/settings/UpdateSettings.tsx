/** Settings → Updates: the running version and a manual update check. */
import { ArrowUpCircle, RotateCw } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { useUpdater } from "@/stores/updater";
import { Button, Spinner } from "@/components/ui";

export function UpdateSettings() {
  const u = useUpdater(
    useShallow((s) => ({ status: s.status, current: s.current, update: s.update, error: s.error })),
  );
  const line =
    u.status === "checking"
      ? "Checking for updates…"
      : u.status === "current"
        ? "You have the latest version."
        : u.update && u.status !== "idle"
          ? `Version ${u.update.version} is available.`
          : u.status === "error"
            ? null
            : "Updates are checked at startup and every 6 hours.";
  return (
    <div className="space-y-2 text-xs">
      <div className="flex items-center gap-3">
        <span className="font-mono text-[11px] text-neutral-500">
          JenderalCode {u.current ? `v${u.current}` : ""}
        </span>
        <span className="text-neutral-600 dark:text-neutral-400">{line}</span>
        <span className="ml-auto flex gap-1.5">
          {u.update && (u.status === "available" || u.status === "error") && (
            <Button size="sm" variant="primary" onClick={() => useUpdater.getState().setDialogOpen(true)}>
              <ArrowUpCircle size={12} /> Update to v{u.update.version}
            </Button>
          )}
          <Button
            size="sm"
            disabled={u.status === "checking" || u.status === "downloading"}
            onClick={() => void useUpdater.getState().checkNow()}
          >
            {u.status === "checking" ? <Spinner className="h-3 w-3" /> : <RotateCw size={12} />} Check for updates
          </Button>
        </span>
      </div>
      {u.status === "error" && u.error && (
        <div className="whitespace-pre-wrap rounded-[4px] bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-500">
          Could not check for updates: {u.error}
        </div>
      )}
    </div>
  );
}
