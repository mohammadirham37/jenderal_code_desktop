/** "Update available" dialog: release notes, download progress, restart. */
import { Download, RotateCw } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { useUpdater } from "@/stores/updater";
import { Button, Modal, Spinner } from "@/components/ui";

const mb = (bytes: number) => `${(bytes / 1_048_576).toFixed(1)} MB`;

export function UpdateDialog() {
  const u = useUpdater(
    useShallow((s) => ({
      open: s.dialogOpen,
      status: s.status,
      current: s.current,
      update: s.update,
      downloaded: s.downloaded,
      total: s.total,
      error: s.error,
    })),
  );
  const busy = u.status === "downloading" || u.status === "restarting";
  const close = () => !busy && useUpdater.getState().setDialogOpen(false);
  const pct = u.total ? Math.min(100, Math.round((u.downloaded / u.total) * 100)) : null;

  return (
    <Modal open={u.open && !!u.update} title="Update available" onClose={close}>
      {u.update && (
        <div className="space-y-3 text-xs">
          <div className="flex items-baseline gap-2">
            <span className="font-display text-lg font-semibold">v{u.update.version}</span>
            {u.current && <span className="font-mono text-[11px] text-neutral-500">you have v{u.current}</span>}
          </div>
          {u.update.body?.trim() && (
            <div>
              <div className="eyebrow mb-1">What's new</div>
              <div className="max-h-64 overflow-y-auto whitespace-pre-wrap rounded-[5px] border border-neutral-200 bg-neutral-50 px-3 py-2 text-[12px] leading-relaxed text-neutral-700 dark:border-neutral-800 dark:bg-neutral-925 dark:text-neutral-300">
                {u.update.body.trim()}
              </div>
            </div>
          )}
          <p className="text-[11px] text-neutral-500">
            The app restarts to finish. Running agent turns and terminals stop; your project, chat and terminal
            tabs come back after the restart (agent CLIs continue their last conversation).
          </p>
          {u.status === "downloading" && (
            <div>
              <div className="h-1.5 overflow-hidden rounded-[2px] bg-neutral-200 dark:bg-neutral-800">
                <div
                  className={`h-full bg-accent-500 transition-[width] ${pct === null ? "w-1/3 animate-pulse" : ""}`}
                  style={pct === null ? undefined : { width: `${pct}%` }}
                />
              </div>
              <div className="mt-1 font-mono text-[10.5px] text-neutral-500">
                Downloading {mb(u.downloaded)}
                {u.total ? ` of ${mb(u.total)} (${pct}%)` : ""}
              </div>
            </div>
          )}
          {u.status === "restarting" && (
            <div className="flex items-center gap-2 text-neutral-500">
              <Spinner className="h-3 w-3" /> Installed, restarting…
            </div>
          )}
          {u.status === "error" && u.error && (
            <div className="whitespace-pre-wrap rounded-[4px] bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-500">
              {u.error}
            </div>
          )}
          <div className="flex justify-end gap-2 pt-1">
            <Button size="sm" disabled={busy} onClick={close}>
              Later
            </Button>
            <Button size="sm" variant="primary" disabled={busy} onClick={() => void useUpdater.getState().install()}>
              {u.status === "error" ? <RotateCw size={12} /> : <Download size={12} />}
              {u.status === "error" ? "Try again" : "Install and restart"}
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}
