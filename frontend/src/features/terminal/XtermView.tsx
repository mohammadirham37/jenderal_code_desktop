/** Shows a registry-owned xterm; refits when the host resizes or becomes
 *  visible, and focuses it when it becomes the active tab. */
import { useEffect, useRef } from "react";
import { useApp } from "@/stores/app";
import { attachXterm, fitXterm, focusXterm } from "./xtermRegistry";

export function XtermView({ terminalId, active }: { terminalId: string; active: boolean }) {
  const fontSize = useApp((s) => s.settings.terminal_font_size);
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    attachXterm(terminalId, host, fontSize);
    const ro = new ResizeObserver(() => fitXterm(terminalId));
    ro.observe(host);
    return () => ro.disconnect();
  }, [terminalId, fontSize]);

  useEffect(() => {
    if (!active) return;
    fitXterm(terminalId);
    focusXterm(terminalId);
  }, [active, terminalId]);

  return (
    <div
      ref={hostRef}
      className="terminal-host absolute inset-0"
      style={{ display: active ? "block" : "none" }}
    />
  );
}
