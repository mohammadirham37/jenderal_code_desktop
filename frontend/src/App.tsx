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
import { Logo } from "@/components/Logo";

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
    <div className="grain flex h-full w-full">
      {/* Sidebar */}
      <aside className="flex w-14 flex-col items-center justify-between border-r border-neutral-200 bg-neutral-100 py-3 dark:border-neutral-800 dark:bg-neutral-925">
        <div className="flex flex-col items-center gap-1">
          <button
            title="JenderalCode — projects"
            onClick={() => useApp.getState().setView("welcome")}
            className="mb-3 transition-transform hover:-rotate-3"
          >
            <Logo size={30} />
          </button>
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
                className={`relative flex h-9 w-9 items-center justify-center rounded-[5px] transition-colors ${
                  active
                    ? "bg-white text-accent-600 shadow-[inset_0_0_0_1px_var(--color-neutral-300)] dark:bg-neutral-850 dark:text-accent-400 dark:shadow-[inset_0_0_0_1px_var(--color-neutral-700)]"
                    : disabled
                      ? "text-neutral-300 dark:text-neutral-700"
                      : "text-neutral-500 hover:text-neutral-900 dark:text-neutral-500 dark:hover:text-neutral-100"
                }`}
              >
                {active && <span className="absolute -left-[9px] h-4 w-[3px] bg-accent-500" />}
                <Icon size={17} />
              </button>
            );
          })}
        </div>
        <div className="flex flex-col items-center gap-1.5">
          {/* Zoom indicator — visible whenever the zoom is not 100%. Click to reset. */}
          {zoom !== 1 && (
            <button
              onClick={() => void useApp.getState().resetZoom()}
              title={`Zoom ${Math.round(zoom * 100)}% — click or press ${isMac ? "⌘0" : "Ctrl+0"} to reset`}
              className="rounded-[4px] border border-neutral-300 bg-white px-1.5 py-0.5 font-mono text-[10px] text-neutral-600 transition-colors hover:border-accent-500 hover:text-accent-600 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-300"
            >
              {Math.round(zoom * 100)}%
            </button>
          )}
          <button
            title="Toggle theme"
            onClick={() =>
              useApp.getState().updateSettings({ theme: settings.theme === "dark" ? "light" : "dark" })
            }
            className="flex h-8 w-8 items-center justify-center rounded-[5px] text-neutral-500 hover:text-neutral-900 dark:hover:text-neutral-100"
          >
            {settings.theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
          </button>
        </div>
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

    </div>
  );
}

const isMac = navigator.platform.toUpperCase().includes("MAC");
