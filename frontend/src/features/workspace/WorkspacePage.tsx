/** Workspace shell: title bar with the Code/Chat layout switch, then either
 *  the IDE layout (explorer | editor + bottom panel | chat) or the
 *  chat-first layout. */
import { useState } from "react";
import { Code2, GitBranch, Globe, MessagesSquare, PanelBottom, PanelBottomClose, Terminal as TerminalIcon } from "lucide-react";
import { BrowserPanel } from "@/features/browser/BrowserPanel";
import { FileExplorer } from "./FileExplorer";
import { ChatPanel } from "@/features/chat/ChatPanel";
import { ChatLayout } from "./ChatLayout";
import { LazyEditorPane, LazyGitPanel, LazyTerminalPanel, PanelSuspense } from "./lazy";
import { useResizable } from "./useResizable";
import { useApp, type LayoutMode } from "@/stores/app";
import { IconButton } from "@/components/ui";

export function WorkspacePage() {
  const workspace = useApp((s) => s.workspace);
  const layout = useApp((s) => s.layout);
  if (!workspace) return null;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-10 shrink-0 items-center gap-2.5 border-b border-neutral-200 px-3 text-[11px] dark:border-neutral-800">
        <span className="font-display text-[14px] font-semibold">{workspace.name}</span>
        <span className="hidden truncate font-mono text-[10.5px] text-neutral-500 md:inline" title={workspace.root_path}>
          {workspace.root_path}
        </span>
        {workspace.is_git_repo && (
          <span className="inline-flex shrink-0 items-center gap-0.5 font-mono text-[10px] text-neutral-500">
            <GitBranch size={10} /> git
          </span>
        )}
        <div className="ml-auto">
          <LayoutSwitch />
        </div>
      </div>
      {layout === "chat" ? <ChatLayout /> : <CodeLayout />}
    </div>
  );
}

const LAYOUTS: { id: LayoutMode; label: string; icon: typeof Code2; hint: string }[] = [
  { id: "chat", label: "Chat", icon: MessagesSquare, hint: "Chat-first: conversations front and center" },
  { id: "code", label: "Code", icon: Code2, hint: "IDE: explorer, editor, terminal, and chat side panel" },
];

function LayoutSwitch() {
  const layout = useApp((s) => s.layout);
  const setLayout = useApp((s) => s.setLayout);
  return (
    <div className="flex items-center rounded-[5px] border border-neutral-300 p-0.5 dark:border-neutral-700" role="tablist">
      {LAYOUTS.map(({ id, label, icon: Icon, hint }) => (
        <button
          key={id}
          role="tab"
          aria-selected={layout === id}
          title={hint}
          onClick={() => setLayout(id)}
          className={`flex items-center gap-1.5 rounded-[3px] px-2.5 py-1 font-mono text-[10px] tracking-wider uppercase transition-colors ${
            layout === id
              ? "bg-neutral-900 text-accent-300 dark:bg-accent-400 dark:text-neutral-950"
              : "text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-200"
          }`}
        >
          <Icon size={12} /> {label}
        </button>
      ))}
    </div>
  );
}

type BottomTab = "terminal" | "git" | "browser";

function CodeLayout() {
  const [bottomTab, setBottomTab] = useState<BottomTab>("terminal");
  const [bottomOpen, setBottomOpen] = useState(true);
  // Mount each bottom tab on first use, then keep it alive so terminal
  // shells survive hiding the panel or switching to Git.
  const [visited, setVisited] = useState<Set<BottomTab>>(() => new Set(["terminal"]));
  const explorer = useResizable("jcode:explorer-width", 230, 140, 420, "right");
  const chat = useResizable("jcode:chat-width", 420, 300, 720, "left");
  const bottom = useResizable("jcode:bottom-height", 240, 120, 600, "up");

  const showTab = (t: BottomTab) => {
    setBottomTab(t);
    setVisited((v) => (v.has(t) ? v : new Set(v).add(t)));
    setBottomOpen(true);
  };

  const tabClass = (t: BottomTab) =>
    `-mb-px flex h-full items-center gap-1.5 border-b-2 px-1 font-mono text-[10.5px] tracking-wide uppercase transition-colors ${
      bottomTab === t
        ? "border-accent-500 text-neutral-900 dark:text-neutral-100"
        : "border-transparent text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-200"
    }`;

  return (
    <div className="flex min-h-0 flex-1">
      <div style={{ width: explorer.size }} className="min-h-0 shrink-0 border-r border-neutral-200 dark:border-neutral-800">
        <FileExplorer />
      </div>
      <div {...explorer.handleProps} className="w-1 shrink-0 cursor-col-resize hover:bg-accent-500/40" />

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="min-h-0 flex-1 overflow-hidden">
          <PanelSuspense>
            <LazyEditorPane />
          </PanelSuspense>
        </div>
        {bottomOpen && <div {...bottom.handleProps} className="h-1 shrink-0 cursor-row-resize hover:bg-accent-500/40" />}
        <div
          style={{ height: bottom.size }}
          className={`min-h-0 shrink-0 flex-col border-t border-neutral-200 dark:border-neutral-800 ${bottomOpen ? "flex" : "hidden"}`}
        >
          <div className="flex h-8 shrink-0 items-stretch gap-3 border-b border-neutral-200 px-2.5 dark:border-neutral-800">
            <button className={tabClass("terminal")} onClick={() => showTab("terminal")}>
              <TerminalIcon size={11} /> Terminal
            </button>
            <button className={tabClass("git")} onClick={() => showTab("git")}>
              <GitBranch size={11} /> Git
            </button>
            <button className={tabClass("browser")} onClick={() => showTab("browser")}>
              <Globe size={11} /> Browser
            </button>
            <IconButton className="ml-auto self-center" title="Hide bottom panel" onClick={() => setBottomOpen(false)}>
              <PanelBottomClose size={13} />
            </IconButton>
          </div>
          <div className="relative min-h-0 flex-1 bg-neutral-50 text-neutral-800 dark:bg-neutral-950 dark:text-neutral-200">
            {[...visited].map((t) => (
              <div key={t} className={`absolute inset-0 ${t === bottomTab ? "block" : "hidden"}`}>
                <PanelSuspense>
                  {t === "terminal" && <LazyTerminalPanel />}
                  {t === "git" && <LazyGitPanel />}
                  {t === "browser" && <BrowserPanel id="code" />}
                </PanelSuspense>
              </div>
            ))}
          </div>
        </div>
        {!bottomOpen && (
          <div className="flex h-7 shrink-0 items-stretch gap-3 border-t border-neutral-200 px-2.5 dark:border-neutral-800">
            <button className={tabClass("terminal")} onClick={() => showTab("terminal")}>
              <PanelBottom size={11} /> Terminal
            </button>
            <button className={tabClass("git")} onClick={() => showTab("git")}>
              <GitBranch size={11} /> Git
            </button>
            <button className={tabClass("browser")} onClick={() => showTab("browser")}>
              <Globe size={11} /> Browser
            </button>
          </div>
        )}
      </div>

      <div {...chat.handleProps} className="w-1 shrink-0 cursor-col-resize hover:bg-accent-500/40" />
      <div style={{ width: chat.size }} className="min-h-0 shrink-0 border-l border-neutral-200 dark:border-neutral-800">
        <ChatPanel variant="side" />
      </div>
    </div>
  );
}
