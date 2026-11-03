/** Chat-first layout: collapsible sidebar (projects, history) | centre column (Chat plus terminal
 *  and agent-CLI tabs) | collapsible context panel (changed files, explorer,
 *  editor, terminal, git). */
import { useEffect, useRef, useState } from "react";
import {
  FileCode2,
  FileDiff,
  FolderTree,
  GitBranch,
  Globe,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Terminal as TerminalIcon,
} from "lucide-react";
import { BrowserPanel } from "@/features/browser/BrowserPanel";
import { FileExplorer } from "./FileExplorer";
import { ChangesPanel } from "./ChangesPanel";
import { CenterColumn } from "./CenterColumn";
import { ChatSidebar } from "./ChatSidebar";
import { LazyEditorPane, LazyGitPanel, LazyTerminalPanel, PanelSuspense } from "./lazy";
import { useEditor } from "@/stores/editor";
import { IconButton } from "@/components/ui";
import { useResizable } from "./useResizable";

type ContextTab = "changes" | "files" | "editor" | "browser" | "terminal" | "git";

const CONTEXT_TABS: { id: ContextTab; label: string; icon: typeof FileDiff }[] = [
  { id: "changes", label: "Changes", icon: FileDiff },
  { id: "files", label: "Files", icon: FolderTree },
  { id: "editor", label: "Editor", icon: FileCode2 },
  { id: "browser", label: "Browser", icon: Globe },
  { id: "terminal", label: "Terminal", icon: TerminalIcon },
  { id: "git", label: "Git", icon: GitBranch },
];

const PANEL_OPEN_KEY = "jcode:chat-context-open";
const SIDEBAR_OPEN_KEY = "jcode:chat-sidebar-open";

export function ChatLayout() {
  const [contextOpen, setContextOpen] = useState(() => localStorage.getItem(PANEL_OPEN_KEY) !== "0");
  const [sidebarOpen, setSidebarOpen] = useState(() => localStorage.getItem(SIDEBAR_OPEN_KEY) !== "0");
  const [tab, setTab] = useState<ContextTab>("changes");
  // Panels are mounted on first visit and then kept alive (hidden), so the
  // terminal's shell sessions survive tab switches.
  const [visited, setVisited] = useState<Set<ContextTab>>(() => new Set(["changes"]));
  const sidebar = useResizable("jcode:chat-sidebar-width", 240, 180, 380, "right");
  const context = useResizable("jcode:chat-context-width", 440, 300, 900, "left");

  const showTab = (t: ContextTab) => {
    setTab(t);
    setVisited((v) => (v.has(t) ? v : new Set(v).add(t)));
    setContextOpen(true);
  };

  useEffect(() => {
    localStorage.setItem(PANEL_OPEN_KEY, contextOpen ? "1" : "0");
  }, [contextOpen]);
  useEffect(() => {
    localStorage.setItem(SIDEBAR_OPEN_KEY, sidebarOpen ? "1" : "0");
  }, [sidebarOpen]);

  // Opening a file or diff from the chat (or explorer) reveals the editor.
  const activeKey = useEditor((s) => s.activeKey);
  const revealTick = useEditor((s) => s.revealTick);
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    if (activeKey) showTab("editor");
  }, [activeKey, revealTick]);

  return (
    <div className="flex min-h-0 flex-1">
      {sidebarOpen && (
        <>
          <div style={{ width: sidebar.size }} className="min-h-0 shrink-0 border-r border-neutral-200 dark:border-neutral-800">
            <ChatSidebar />
          </div>
          <div {...sidebar.handleProps} className="w-1 shrink-0 cursor-col-resize hover:bg-accent-500/40" />
        </>
      )}

      <div className="relative min-w-0 flex-1">
        <CenterColumn
          leading={
            <IconButton
              title={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
              onClick={() => setSidebarOpen(!sidebarOpen)}
            >
              {sidebarOpen ? <PanelLeftClose size={14} /> : <PanelLeftOpen size={14} />}
            </IconButton>
          }
          trailing={
            <IconButton
              title={contextOpen ? "Hide context panel" : "Show context panel"}
              onClick={() => setContextOpen(!contextOpen)}
            >
              {contextOpen ? <PanelRightClose size={14} /> : <PanelRightOpen size={14} />}
            </IconButton>
          }
        />
      </div>

      {contextOpen && (
        <div {...context.handleProps} className="w-1 shrink-0 cursor-col-resize hover:bg-accent-500/40" />
      )}
      <div
        style={{ width: context.size }}
        className={`min-h-0 shrink-0 flex-col border-l border-neutral-200 dark:border-neutral-800 ${
          contextOpen ? "flex" : "hidden"
        }`}
      >
        <div className="flex h-10 shrink-0 items-stretch gap-3 overflow-x-auto border-b border-neutral-200 px-3 [scrollbar-width:none] dark:border-neutral-800">
          {CONTEXT_TABS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => showTab(id)}
              title={label}
              className={`-mb-px flex shrink-0 items-center gap-1.5 border-b-2 font-mono text-[10.5px] tracking-wide uppercase transition-colors ${
                tab === id
                  ? "border-accent-500 text-neutral-900 dark:text-neutral-100"
                  : "border-transparent text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-200"
              }`}
            >
              <Icon size={12} /> {label}
            </button>
          ))}
        </div>
        <div className="relative min-h-0 flex-1">
          {[...visited].map((t) => (
            <div key={t} className={`absolute inset-0 ${t === tab ? "block" : "hidden"}`}>
              <PanelSuspense>
                {t === "changes" && <ChangesPanel onOpen={() => showTab("editor")} />}
                {t === "files" && <FileExplorer />}
                {t === "editor" && <LazyEditorPane />}
                {t === "browser" && <BrowserPanel id="chat" />}
                {t === "terminal" && <LazyTerminalPanel />}
                {t === "git" && <LazyGitPanel />}
              </PanelSuspense>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}


