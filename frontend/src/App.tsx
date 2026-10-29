/** App shell: sidebar navigation + routed views + command palette. */
import { lazy, Suspense, useEffect } from "react";
import {
  Boxes,
  Plug,
  Bot,
  Clock,
  Settings as SettingsIcon,
  MessageSquareCode,
  Moon,
  Sun,
} from "lucide-react";
import { useApp, type View } from "@/stores/app";
import { useChat } from "@/stores/chat";
import { useProviders } from "@/stores/providers";
import { useEditor } from "@/stores/editor";
import { CommandPalette } from "@/components/CommandPalette";
import { Welcome } from "@/features/welcome/Welcome";
import { WorkspacePage } from "@/features/workspace/WorkspacePage";
import { Spinner } from "@/components/ui";

// Secondary views load on demand to keep the startup bundle small.
const named = <K extends string>(p: Promise<Record<K, React.ComponentType>>, k: K) =>
  p.then((m) => ({ default: m[k] }));
const ProviderCenter = lazy(() => named(import("@/features/providers/ProviderCenter"), "ProviderCenter"));
const AgentStudio = lazy(() => named(import("@/features/agents/AgentStudio"), "AgentStudio"));
const SessionsPage = lazy(() => named(import("@/features/sessions/SessionsPage"), "SessionsPage"));
const SettingsPage = lazy(() => named(import("@/features/settings/SettingsPage"), "SettingsPage"));
const EditorScrollTest = lazy(() => named(import("@/features/workspace/EditorScrollTest"), "EditorScrollTest"));

const NAV: { view: View; label: string; icon: typeof Boxes; needsWorkspace?: boolean }[] = [
  { view: "welcome", label: "Projects", icon: Boxes },
  { view: "workspace", label: "Workspace", icon: MessageSquareCode, needsWorkspace: true },
  { view: "providers", label: "Providers", icon: Plug },
  { view: "agents", label: "Agents", icon: Bot },
  { view: "sessions", label: "Sessions", icon: Clock },
  { view: "settings", label: "Settings", icon: SettingsIcon },
];

export default function App() {
  // Standalone editor layout harness (dev builds only): open with `#editor-test`.
  if (import.meta.env.DEV && window.location.hash === "#editor-test") {
    return (
      <Suspense fallback={null}>
        <EditorScrollTest />
      </Suspense>
    );
  }
  return <AppShell />;
}

function AppShell() {
  const view = useApp((s) => s.view);
  const workspace = useApp((s) => s.workspace);
  const settings = useApp((s) => s.settings);
  const init = useApp((s) => s.init);
  const zoom = useApp((s) => s.zoom);
  const chatInit = useChat((s) => s.init);
  const loadProviders = useProviders((s) => s.load);

  useEffect(() => {
    void init();
    void chatInit();
    void loadProviders();
  }, [init, chatInit, loadProviders]);

  // Global save shortcut.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void useEditor.getState().saveActive();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Zoom shortcuts: Cmd/Ctrl + = (in), - (out), 0 (reset). Uses `e.code` so
  // it works across keyboard layouts and the numpad.
  useEffect(() => {
    const onZoomKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      const app = useApp.getState();
      switch (e.code) {
        case "Equal":
        case "NumpadAdd":
          e.preventDefault();
          void app.zoomIn();
          break;
        case "Minus":
        case "NumpadSubtract":
          e.preventDefault();
          void app.zoomOut();
          break;
        case "Digit0":
        case "Numpad0":
          e.preventDefault();
          void app.resetZoom();
          break;
      }
    };
    window.addEventListener("keydown", onZoomKey);
    return () => window.removeEventListener("keydown", onZoomKey);
  }, []);

  return (
    <div className="flex h-full w-full">
      {/* Sidebar */}
      <aside className="flex w-14 flex-col items-center justify-between border-r border-neutral-200 dark:border-neutral-800 bg-neutral-50 dark:bg-[#101218] py-3">
        <div className="flex flex-col items-center gap-1">
          <div
            title="JenderalCode"
            className="mb-2 flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-br from-indigo-500 to-violet-600 font-bold text-white shadow"
          >
            J
          </div>
          {NAV.map((item) => {
            const disabled = item.needsWorkspace && !workspace;
            const active = view === item.view;
            const Icon = item.icon;
            return (
              <button
                key={item.view}
                title={disabled ? `${item.label} — open a project first` : item.label}
                aria-label={item.label}
                aria-current={active ? "page" : undefined}
                disabled={disabled}
                onClick={() => useApp.getState().setView(item.view)}
                className={`relative flex h-9 w-9 items-center justify-center rounded-lg transition-colors ${
                  active
                    ? "bg-indigo-500/15 text-indigo-500"
                    : disabled
                      ? "text-neutral-300 dark:text-neutral-700"
                      : "text-neutral-500 hover:bg-neutral-200/70 dark:text-neutral-400 dark:hover:bg-neutral-800"
                }`}
              >
                {active && <span className="absolute -left-2.5 h-5 w-0.5 rounded-r bg-indigo-500" />}
                <Icon size={17} />
              </button>
            );
          })}
        </div>
        <button
          title="Toggle theme"
          onClick={() =>
            useApp.getState().updateSettings({ theme: settings.theme === "dark" ? "light" : "dark" })
          }
          className="flex h-8 w-8 items-center justify-center rounded-lg text-neutral-500 hover:bg-neutral-200/70 dark:text-neutral-400 dark:hover:bg-neutral-800"
        >
          {settings.theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
        </button>
      </aside>

      {/* Main view */}
      <main className="relative flex min-w-0 flex-1 flex-col overflow-hidden">
        {view === "welcome" && <Welcome />}
        {view === "workspace" && (workspace ? <WorkspacePage /> : <Welcome />)}
        <Suspense
          fallback={
            <div className="flex h-full items-center justify-center text-neutral-500">
              <Spinner />
            </div>
          }
        >
          {view === "providers" && <ProviderCenter />}
          {view === "agents" && <AgentStudio />}
          {view === "sessions" && <SessionsPage />}
          {view === "settings" && <SettingsPage />}
        </Suspense>
      </main>

      <CommandPalette />

      {/* Zoom indicator — visible whenever the zoom is not 100%. Click to reset. */}
      {zoom !== 1 && (
        <button
          onClick={() => void useApp.getState().resetZoom()}
          title={`Zoom ${Math.round(zoom * 100)}% — click or press ${isMac ? "⌘0" : "Ctrl+0"} to reset`}
          className="fixed bottom-3 right-3 z-40 rounded-full border border-neutral-300 bg-white/90 px-2.5 py-1 text-[11px] font-medium text-neutral-600 shadow-md backdrop-blur transition-colors hover:border-indigo-400 hover:text-indigo-500 dark:border-neutral-700 dark:bg-[#15171d]/90 dark:text-neutral-300"
        >
          {Math.round(zoom * 100)}%
        </button>
      )}
    </div>
  );
}

const isMac = navigator.platform.toUpperCase().includes("MAC");
