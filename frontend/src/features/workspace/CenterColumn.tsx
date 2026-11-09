/** Chat layout centre column: the built-in Chat tab plus terminal and
 *  agent-CLI tabs opened from the "+" menu. */
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useShallow } from "zustand/react/shallow";
import { Bot, Download, MessageSquare, Plus, RotateCcw, Settings, ShieldOff, Smartphone, Terminal as TerminalIcon, X } from "lucide-react";
import { ChatPanel } from "@/features/chat/ChatPanel";
import { useApp } from "@/stores/app";
import { CHAT_TAB, useCenterTabs, type CenterTab } from "@/stores/centerTabs";
import { Button, IconButton } from "@/components/ui";
import { LazyXtermView, PanelSuspense } from "./lazy";
import { AgentUsageButton } from "./AgentUsage";
import { confirmAction } from "@/lib/confirm";

export function CenterColumn({ leading, trailing }: { leading?: ReactNode; trailing?: ReactNode }) {
  const { tabs, activeId } = useCenterTabs(useShallow((s) => ({ tabs: s.tabs, activeId: s.activeId })));
  const showChat = activeId === CHAT_TAB;
  useNewTerminalShortcut();

  return (
    <div className="flex h-full min-w-0 flex-col">
      <div className="flex h-9 shrink-0 items-stretch border-b border-neutral-200 pl-1 pr-2 dark:border-neutral-800">
        {leading && <div className="mr-1 flex items-center">{leading}</div>}
        <div className="flex min-w-0 items-stretch overflow-x-auto [scrollbar-width:none]">
          <TabButton
            active={showChat}
            icon={<MessageSquare size={12} />}
            label="Chat"
            onClick={() => useCenterTabs.getState().setActive(CHAT_TAB)}
          />
          {tabs.map((t) => (
            <TabButton
              key={t.id}
              active={t.id === activeId}
              icon={
                t.bypass ? (
                  <ShieldOff size={12} className="text-red-500" />
                ) : t.remote ? (
                  <Smartphone size={12} />
                ) : t.agentId ? (
                  <Bot size={12} />
                ) : (
                  <TerminalIcon size={12} />
                )
              }
              label={t.title}
              title={
                [
                  t.remote && `${t.title}: opened from the phone (sized like the phone)`,
                  t.bypass && `${t.title}: permission checks bypassed`,
                ]
                  .filter(Boolean)
                  .join(" · ") || undefined
              }
              dim={t.exited}
              onClick={() => useCenterTabs.getState().setActive(t.id)}
              onClose={() => void confirmClose(t)}
            />
          ))}
        </div>
        <NewTabMenu />
        <AgentUsageButton />
        <div className="ml-auto flex items-center">{trailing}</div>
      </div>
      <div className="relative min-h-0 flex-1">
        <div className={`absolute inset-0 ${showChat ? "block" : "hidden"}`}>
          <ChatPanel variant="full" />
        </div>
        {tabs.length > 0 && (
          <div className={`absolute inset-0 bg-neutral-50 text-neutral-800 dark:bg-neutral-950 dark:text-neutral-200 ${showChat ? "hidden" : "block"}`}>
            <PanelSuspense>
              {tabs.map((t) => (
                <TerminalTabBody key={t.id} tab={t} active={!showChat && t.id === activeId} />
              ))}
            </PanelSuspense>
          </div>
        )}
      </div>
    </div>
  );
}

async function confirmClose(tab: CenterTab) {
  if (tab.agentId && !tab.exited && !await confirmAction(`Stop ${tab.title} and close its tab?`)) return;
  void useCenterTabs.getState().close(tab.id);
}

function TerminalTabBody({ tab, active }: { tab: CenterTab; active: boolean }) {
  return (
    <div className={`absolute inset-0 flex-col ${active ? "flex" : "hidden"}`}>
      {tab.exited && (
        <div className="flex shrink-0 items-center gap-2 border-b border-neutral-200 px-3 py-1 font-mono text-[11px] text-neutral-500 dark:border-neutral-800 dark:text-neutral-400">
          <span className={`min-w-0 flex-1 truncate ${tab.error ? "text-red-600 dark:text-red-400" : ""}`}>
            {tab.error ?? "Process exited"}
          </span>
          <Button size="sm" variant="ghost" onClick={() => void useCenterTabs.getState().restart(tab.id)}>
            <RotateCcw size={11} /> Restart
          </Button>
        </div>
      )}
      <div className="relative min-h-0 flex-1">
        <LazyXtermView terminalId={tab.id} active={active} />
      </div>
    </div>
  );
}

function TabButton({
  active,
  icon,
  label,
  dim,
  title,
  onClick,
  onClose,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  dim?: boolean;
  title?: string;
  onClick: () => void;
  onClose?: () => void;
}) {
  return (
    <div
      onClick={onClick}
      title={title}
      className={`group -mb-px flex shrink-0 cursor-pointer items-center gap-1.5 border-b-2 px-2.5 font-mono text-[10.5px] tracking-wide transition-colors ${
        active
          ? "border-accent-500 text-neutral-900 dark:text-neutral-100"
          : "border-transparent text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-200"
      } ${dim ? "opacity-60" : ""}`}
    >
      {icon}
      <span className="max-w-[140px] truncate">{label}</span>
      {onClose && (
        <button
          title="Close tab"
          onClick={(e) => {
            e.stopPropagation();
            onClose();
          }}
          className="rounded p-0.5 opacity-0 transition-opacity group-hover:opacity-100 hover:bg-neutral-300/60 dark:hover:bg-neutral-700"
        >
          <X size={10} />
        </button>
      )}
    </div>
  );
}

function NewTabMenu() {
  const [open, setOpen] = useState(false);
  const agents = useApp((s) => s.settings.agent_clis);
  const installed = useCenterTabs((s) => s.installed);
  const ref = useRef<HTMLDivElement>(null);
  const enabled = agents.filter((a) => a.enabled && a.command.trim());

  useEffect(() => {
    if (!open) return;
    if (useCenterTabs.getState().installed === null) void useCenterTabs.getState().detectInstalled();
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

  const run = (action: () => void) => {
    setOpen(false);
    action();
  };

  return (
    <div ref={ref} className="relative flex items-center">
      <IconButton title="New tab" className="ml-0.5 h-6 w-6" onClick={() => setOpen((o) => !o)}>
        <Plus size={13} />
      </IconButton>
      {open && (
        <div className="animate-rise absolute left-0 top-full z-30 mt-1 w-60 rounded-[6px] border border-neutral-200 bg-white p-1 shadow-lg dark:border-neutral-800 dark:bg-neutral-900">
          <MenuItem
            icon={<TerminalIcon size={13} />}
            label="New Terminal"
            hint="⌘T"
            onClick={() => run(() => void useCenterTabs.getState().openTerminal())}
          />
          {enabled.length > 0 && <MenuDivider />}
          {enabled.map((a) => {
            const missing = installed?.[a.id] === false;
            const bypass = a.bypass_args?.trim();
            const installer = missing ? a.install_command?.trim() : undefined;
            return (
              <div key={a.id} className="group/agent flex items-center gap-0.5">
                <div className="min-w-0 flex-1">
                  <MenuItem
                    icon={<Bot size={13} />}
                    label={a.name}
                    hint={missing ? "not installed" : installed === null ? "…" : undefined}
                    dim={missing}
                    onClick={() => run(() => void useCenterTabs.getState().openAgent(a))}
                  />
                </div>
                {installer && (
                  <button
                    title={`Install ${a.name}: ${installer}`}
                    onClick={() =>
                      run(() => {
                        void confirmAction(
                          `Install ${a.name}?\n\nThis runs in a new terminal tab:\n${installer}\n\nThe ${a.name} menu entry is checked again when it finishes.`,
                        ).then((ok) => {
                          if (ok) void useCenterTabs.getState().installAgent(a);
                        });
                      })
                    }
                    className="flex shrink-0 items-center gap-1 rounded-[4px] px-1.5 py-1.5 font-mono text-[10px] text-accent-600 transition-colors hover:bg-accent-500/10 dark:text-accent-400"
                  >
                    <Download size={12} /> install
                  </button>
                )}
                {bypass && !installer && (
                  <button
                    title={`Open with bypass permissions: ${a.command} ${bypass}`}
                    onClick={() => run(() => void useCenterTabs.getState().openAgent(a, true))}
                    className="flex shrink-0 items-center gap-1 rounded-[4px] px-1.5 py-1.5 font-mono text-[10px] text-neutral-500 transition-colors hover:bg-red-500/10 hover:text-red-500"
                  >
                    <ShieldOff size={12} /> bypass
                  </button>
                )}
              </div>
            );
          })}
          <MenuDivider />
          <MenuItem icon={<Settings size={13} />} label="Agent settings…" dim onClick={() => run(openAgentSettings)} />
        </div>
      )}
    </div>
  );
}

function MenuItem({
  icon,
  label,
  hint,
  dim,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  hint?: string;
  dim?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex w-full items-center gap-2 rounded-[4px] px-2 py-1.5 text-left text-xs transition-colors hover:bg-neutral-100 dark:hover:bg-neutral-800 ${
        dim ? "text-neutral-500" : "text-neutral-800 dark:text-neutral-200"
      }`}
    >
      <span className="shrink-0 text-neutral-500">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {hint && <span className="shrink-0 font-mono text-[10px] text-neutral-500">{hint}</span>}
    </button>
  );
}

function MenuDivider() {
  return <div className="my-1 h-px bg-neutral-200 dark:bg-neutral-800" />;
}

function openAgentSettings() {
  useApp.getState().setView("settings");
  setTimeout(() => document.getElementById("agent-clis")?.scrollIntoView({ block: "start" }), 50);
}

/** ⌘T (Ctrl+Shift+T elsewhere) opens a terminal tab, except while typing in
 *  a text field. Terminals count as "not typing" (xterm uses a textarea). */
function useNewTerminalShortcut() {
  useEffect(() => {
    const isMac = navigator.platform.toUpperCase().includes("MAC");
    const onKey = (e: KeyboardEvent) => {
      const combo = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && e.shiftKey;
      if (!combo || e.altKey || e.code !== "KeyT") return;
      const el = e.target as HTMLElement | null;
      if (el?.closest("input, textarea, [contenteditable='true']") && !el.closest(".xterm")) return;
      e.preventDefault();
      void useCenterTabs.getState().openTerminal();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
