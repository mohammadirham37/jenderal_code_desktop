/** Title-bar indicator while remote control is active (a paired phone can
 *  reach this desktop). Opens Settings → Remote control. */
import { ShieldOff, Smartphone } from "lucide-react";
import { useApp } from "@/stores/app";
import { useRemote } from "@/stores/remote";

export function RemoteBadge() {
  const status = useRemote((s) => s.status);
  if (!status?.enabled || status.devices.length === 0 || !["online", "connecting", "offline"].includes(status.state)) {
    return null;
  }
  const phonesOnline = status.devices.filter((d) => d.online).length;
  const live = status.state === "online";
  const title =
    (live
      ? `Remote control is on: ${phonesOnline} of ${status.devices.length} phone(s) online`
      : "Remote control is on, but the relay cannot be reached") +
    (status.allow_full_access ? ". Phones may use Full access, terminals and file/git writes." : "");
  return (
    <button
      title={title}
      onClick={() => {
        useApp.getState().setView("settings");
        setTimeout(() => document.getElementById("remote")?.scrollIntoView({ block: "start" }), 50);
      }}
      className={`flex items-center gap-1.5 rounded-[5px] border px-2 py-1 font-mono text-[10px] uppercase tracking-wider transition-colors ${
        live
          ? "border-accent-500/60 bg-accent-500/10 text-accent-700 hover:bg-accent-500/20 dark:text-accent-300"
          : "border-amber-500/50 bg-amber-500/10 text-amber-700 dark:text-amber-300"
      }`}
    >
      <Smartphone size={12} /> Remote{phonesOnline > 0 ? ` · ${phonesOnline}` : ""}
      {status.allow_full_access && <ShieldOff size={12} className="text-red-500" />}
    </button>
  );
}
