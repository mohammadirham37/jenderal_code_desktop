/** Settings → Remote control: on/off, the fixed relay, what phones may do,
 *  QR pairing with the 6-digit check, and the list of paired phones
 *  (docs/REMOTE_PROTOCOL.md). */
import { useEffect, useState } from "react";
import { Download, ExternalLink, QrCode, Smartphone, Trash2 } from "lucide-react";
import { api, events, type RemotePairingEvent, type RemotePairingStarted, type RemoteState } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { useRemote } from "@/stores/remote";
import { Button, Spinner, Toggle, inputClass } from "@/components/ui";
import { confirmAction } from "@/lib/confirm";

const STATE_LABEL: Record<RemoteState, string> = {
  off: "Off",
  no_relay: "No relay set",
  unpaired: "No phone paired",
  connecting: "Connecting…",
  online: "Connected to the relay",
  offline: "Relay unreachable, retrying",
  stopped: "Stopped",
};

function dotClass(state: RemoteState) {
  if (state === "online") return "bg-accent-500";
  if (state === "connecting" || state === "offline") return "bg-amber-500";
  if (state === "stopped") return "bg-red-500";
  return "bg-neutral-400";
}

function formatWhen(ms?: number | null) {
  return ms ? new Date(ms).toLocaleString() : "never";
}

export function RemoteSettings() {
  const settings = useApp((s) => s.settings);
  const updateSettings = useApp((s) => s.updateSettings);
  const status = useRemote((s) => s.status);
  const [error, setError] = useState<string | null>(null);
  const [pairing, setPairing] = useState<RemotePairingStarted | null>(null);
  const [outcome, setOutcome] = useState<RemotePairingEvent | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void useRemote.getState().refresh();
    const un = events.remotePairing((ev) => setOutcome(ev));
    return () => void un.then((f) => f());
  }, []);

  const devices = status?.devices ?? [];
  const state = status?.state ?? "off";

  const setEnabled = async (on: boolean) => {
    setError(null);
    await updateSettings({ remote_enabled: on });
  };

  const setAllowFull = async (on: boolean) => {
    if (
      on &&
      !(await confirmAction(
        "Allow Full access & terminal from paired phones?\n\nA phone can then switch sessions to Full access (the agent runs every command without asking), open terminals on this computer, edit files and run git commits. Anyone holding a paired, unlocked phone could control this computer.",
      ))
    )
      return;
    await updateSettings({ remote_allow_full_access: on });
  };

  const startPairing = async () => {
    setError(null);
    setOutcome(null);
    setBusy(true);
    try {
      setPairing(await api.remoteStartPairing());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const closePairing = async () => {
    if (pairing && outcome?.state !== "approved") await api.remoteCancelPairing().catch(() => {});
    if (pairing && outcome?.state === "approved") {
      await api.remoteConfirmPairing(pairing.pairing_id, false).catch(() => {});
    }
    setPairing(null);
    setOutcome(null);
  };

  const confirm = async (accept: boolean) => {
    if (!pairing) return;
    setBusy(true);
    try {
      useRemote.getState().setStatus(await api.remoteConfirmPairing(pairing.pairing_id, accept));
      setPairing(null);
      setOutcome(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (id: string, name: string) => {
    if (!(await confirmAction(`Remove "${name}"? It can no longer control this desktop.`))) return;
    try {
      useRemote.getState().setStatus(await api.remoteRevokeDevice(id));
    } catch (e) {
      setError(String(e));
    }
  };

  const forget = async () => {
    if (
      !(await confirmAction(
        "Unregister this desktop from the relay?\n\nAll paired phones are removed. You can pair again later.",
      ))
    )
      return;
    try {
      useRemote.getState().setStatus(await api.remoteForgetDesktop());
    } catch (e) {
      setError(String(e));
    }
  };

  const canPair = settings.remote_enabled && state !== "no_relay";

  return (
    <div className="space-y-3 text-xs">
      <MobileAppCard />

      <div className="flex items-center gap-3">
        <Toggle checked={settings.remote_enabled} onChange={(v) => void setEnabled(v)} label="Allow remote control" />
        <span className="ml-auto flex items-center gap-1.5 font-mono text-[10.5px] text-neutral-500">
          <span className={`h-1.5 w-1.5 rounded-full ${dotClass(state)}`} />
          {STATE_LABEL[state]}
        </span>
      </div>
      {status?.error && (
        <div className="whitespace-pre-wrap rounded-[4px] bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-500">
          {status.error}
        </div>
      )}

      <div className="flex items-center gap-2 text-[11px] text-neutral-500">
        <span className="eyebrow">Relay</span>
        <span className="font-mono text-neutral-700 dark:text-neutral-300">{status?.relay_url ?? "…"}</span>
      </div>

      <div className="rounded-[5px] border border-neutral-200 px-3 py-2 dark:border-neutral-800">
        <Toggle
          checked={settings.remote_allow_full_access}
          onChange={(v) => void setAllowFull(v)}
          label="Allow Full access & terminal from phone"
        />
        <p className="mt-1 text-[11px] text-neutral-500">
          Off: phones can chat, answer approvals and view files, diffs and git, but cannot use Full access, open
          terminals, edit files or run git writes. On: all of that from the phone.
        </p>
      </div>

      <div className="flex items-center gap-2">
        <Button size="sm" variant="primary" disabled={!canPair || busy} onClick={() => void startPairing()}>
          {busy && !pairing ? <Spinner className="h-3 w-3" /> : <QrCode size={12} />} Pair a phone
        </Button>
        {!settings.remote_enabled && <span className="text-[11px] text-neutral-500">Turn remote control on to pair.</span>}
      </div>

      {devices.length > 0 && (
        <ul className="divide-y divide-neutral-200 rounded-[5px] border border-neutral-200 dark:divide-neutral-800 dark:border-neutral-800">
          {devices.map((d) => (
            <li key={d.device_id} className="flex items-center gap-2.5 px-3 py-2">
              <Smartphone size={14} className="shrink-0 text-neutral-500" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5">
                  <span className="truncate font-medium">{d.name || "Phone"}</span>
                  <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${d.online ? "bg-accent-500" : "bg-neutral-400"}`} />
                </div>
                <div className="truncate font-mono text-[10px] text-neutral-500">
                  {d.user_email} · {d.online ? "online" : `last seen ${formatWhen(d.last_seen)}`}
                </div>
              </div>
              <Button size="sm" variant="ghost" title="Remove this phone" onClick={() => void revoke(d.device_id, d.name)}>
                <Trash2 size={12} />
              </Button>
            </li>
          ))}
        </ul>
      )}

      {error && (
        <div className="whitespace-pre-wrap rounded-[4px] bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-500">{error}</div>
      )}

      {(devices.length > 0 || state === "stopped") && (
        <button className="text-[11px] text-neutral-500 underline-offset-2 hover:underline" onClick={() => void forget()}>
          Unregister this desktop from the relay
        </button>
      )}

      {pairing && (
        <PairingDialog
          pairing={pairing}
          outcome={outcome}
          busy={busy}
          onClose={() => void closePairing()}
          onRetry={() => void startPairing()}
          onConfirm={(accept) => void confirm(accept)}
        />
      )}
    </div>
  );
}

/** Where to get the Android app: QR to scan with the phone, or open in the browser. */
function MobileAppCard() {
  const [dl, setDl] = useState<{ url: string; qr_svg: string } | null>(null);
  const [showQr, setShowQr] = useState(false);
  useEffect(() => {
    void api.remoteMobileDownload().then(setDl).catch(() => {});
  }, []);
  if (!dl) return null;
  return (
    <div className="flex gap-3 rounded-[5px] border border-neutral-200 px-3 py-2.5 dark:border-neutral-800">
      <Smartphone size={16} className="mt-0.5 shrink-0 text-accent-500" />
      <div className="min-w-0 flex-1">
        <div className="font-medium">JenderalCode Mobile for Android</div>
        <p className="mt-0.5 text-[11px] text-neutral-500">
          Install the phone app first, then pair it here. Scan the code with your phone&apos;s camera to download it.
        </p>
        <div className="mt-1 truncate font-mono text-[10.5px] text-neutral-500">{dl.url}</div>
        <div className="mt-2 flex gap-1.5">
          <Button size="sm" onClick={() => setShowQr((v) => !v)}>
            <QrCode size={12} /> {showQr ? "Hide QR code" : "Show QR code"}
          </Button>
          <Button size="sm" onClick={() => void api.remoteOpenMobileDownload().catch(() => {})}>
            <ExternalLink size={12} /> Open download page
          </Button>
        </div>
      </div>
      {showQr && (
        <img
          alt="Download JenderalCode Mobile"
          className="h-28 w-28 shrink-0 rounded-[4px] bg-white p-1"
          src={`data:image/svg+xml;utf8,${encodeURIComponent(dl.qr_svg)}`}
        />
      )}
    </div>
  );
}

function useCountdown(until: number) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return Math.max(0, Math.round((until - now) / 1000));
}

function PairingDialog({
  pairing,
  outcome,
  busy,
  onClose,
  onRetry,
  onConfirm,
}: {
  pairing: RemotePairingStarted;
  outcome: RemotePairingEvent | null;
  busy: boolean;
  onClose: () => void;
  onRetry: () => void;
  onConfirm: (accept: boolean) => void;
}) {
  const left = useCountdown(pairing.expires_at);
  const mine = outcome && outcome.pairing_id === pairing.pairing_id ? outcome : null;
  const sas = mine?.sas ? `${mine.sas.slice(0, 3)} ${mine.sas.slice(3)}` : null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={onClose}>
      <div
        className="ticks w-full max-w-sm animate-rise rounded-md border border-neutral-300 bg-white p-5 shadow-2xl dark:border-neutral-700 dark:bg-neutral-900"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="eyebrow mb-3">Pair a phone</div>
        {!mine && (
          <>
            <img
              alt="Pairing QR code"
              className="mx-auto h-60 w-60 rounded-[4px] bg-white p-1"
              src={`data:image/svg+xml;utf8,${encodeURIComponent(pairing.qr_svg)}`}
            />
            <p className="mt-3 text-center text-xs text-neutral-600 dark:text-neutral-400">
              Scan with the JenderalCode app. {left > 0 ? `Expires in ${left}s.` : "Expired."}
            </p>
            <p className="mt-1 text-center text-[11px] text-neutral-500">
              No app yet?{" "}
              <button
                className="inline-flex items-center gap-0.5 text-accent-600 underline-offset-2 hover:underline dark:text-accent-300"
                onClick={() => void api.remoteOpenMobileDownload().catch(() => {})}
              >
                <Download size={11} /> Get JenderalCode Mobile
              </button>
            </p>
            {left === 0 && (
              <div className="mt-3 flex justify-center">
                <Button size="sm" onClick={onRetry}>
                  New code
                </Button>
              </div>
            )}
          </>
        )}
        {mine?.state === "approved" && (
          <>
            <p className="text-xs text-neutral-600 dark:text-neutral-400">
              <span className="font-medium text-neutral-900 dark:text-neutral-100">{mine.mobile_name || "A phone"}</span>{" "}
              ({mine.user_email}) wants to control this desktop. Check that the phone shows the same code:
            </p>
            <div className="my-4 text-center font-mono text-3xl tracking-[0.2em] text-accent-600 dark:text-accent-300">{sas}</div>
            <p className="mb-4 text-[11px] text-neutral-500">
              Only allow it if the codes match. The phone can then send messages and answer approvals in your projects.
              Sessions with full access stay desktop-only.
            </p>
            <div className="flex justify-end gap-2">
              <Button size="sm" disabled={busy} onClick={() => onConfirm(false)}>
                Codes differ — reject
              </Button>
              <Button size="sm" variant="primary" disabled={busy} onClick={() => onConfirm(true)}>
                Allow
              </Button>
            </div>
          </>
        )}
        {(mine?.state === "expired" || mine?.state === "error") && (
          <>
            <p className="text-xs text-neutral-600 dark:text-neutral-400">
              {mine.state === "expired" ? "The code expired before the phone confirmed it." : `Pairing failed: ${mine.error}`}
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <Button size="sm" onClick={onClose}>
                Close
              </Button>
              <Button size="sm" variant="primary" onClick={onRetry}>
                Try again
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
