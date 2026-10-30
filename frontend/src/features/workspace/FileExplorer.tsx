/** File explorer tree with lazy directory loading. */
import { useCallback, useEffect, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  File as FileIcon,
  FileJson,
  FileCode,
  FileText,
  Folder as FolderIcon,
  RefreshCw,
  FilePlus,
  Eye,
  EyeOff,
  FolderPlus,
  Trash2,
} from "lucide-react";
import { api, events, type FileEntry } from "@/lib/ipc";
import { useEditor } from "@/stores/editor";
import { useApp } from "@/stores/app";
import { languageOf } from "@/lib/format";
import { IconButton } from "@/components/ui";
import { confirmAction } from "@/lib/confirm";

function iconFor(name: string, isDir: boolean) {
  if (isDir) return <FolderIcon size={13} className="text-accent-600 dark:text-accent-400" />;
  const lang = languageOf(name);
  if (["json"].includes(lang)) return <FileJson size={13} className="text-amber-400" />;
  if (["typescript", "tsx", "javascript", "jsx", "rust", "python"].includes(lang))
    return <FileCode size={13} className="text-sky-400" />;
  if (["markdown", "text"].includes(lang)) return <FileText size={13} className="text-neutral-400" />;
  return <FileIcon size={13} className="text-neutral-400" />;
}

export function FileExplorer() {
  const root = useApp((s) => s.workspace?.root_path ?? "");
  const showHidden = useApp((s) => s.settings.show_hidden_files);
  const expandedDirs = useEditor((s) => s.expandedDirs);
  const toggleDir = useEditor((s) => s.toggleDir);
  const openFile = useEditor((s) => s.openFile);
  const [tree, setTree] = useState<Record<string, FileEntry[]>>({});
  const [selected, setSelected] = useState<string | null>(null);

  const loadDir = useCallback(
    async (path: string) => {
      const entries = await api.fsList(path || undefined, showHidden);
      setTree((t) => ({ ...t, [path]: entries }));
    },
    [showHidden],
  );

  useEffect(() => {
    if (!root) return;
    setTree({});
    // Reload the root and every expanded folder (e.g. after toggling
    // hidden files), so open folders don't render empty.
    for (const d of ["", ...Array.from(useEditor.getState().expandedDirs)]) void loadDir(d).catch(() => {});
  }, [root, loadDir]);

  // Refresh affected directories on fs change events.
  useEffect(() => {
    let unlisten: Promise<() => void> | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    unlisten = events.fsChanged(() => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        const dirs = ["", ...Array.from(useEditor.getState().expandedDirs)];
        for (const d of dirs) void loadDir(d);
      }, 150);
    });
    return () => {
      void unlisten?.then((f) => f());
      if (timer) clearTimeout(timer);
    };
  }, [loadDir]);

  const renderDir = (path: string, depth: number): React.ReactNode => {
    const entries = tree[path];
    if (!entries) return null;
    return entries.map((e) => {
      const key = e.path;
      if (e.is_dir) {
        const expanded = expandedDirs.has(key);
        return (
          <div key={key}>
            <button
              className={`group flex w-full items-center gap-1 rounded px-1 py-[3px] text-left text-xs hover:bg-neutral-200/70 dark:hover:bg-neutral-800/70 ${
                selected === key ? "bg-accent-500/10" : ""
              }`}
              style={{ paddingLeft: depth * 12 + 4 }}
              onClick={() => {
                setSelected(key);
                toggleDir(key);
                if (!expanded && !tree[key]) void loadDir(key);
              }}
            >
              {expanded ? (
                <ChevronDown size={12} className="shrink-0 text-neutral-500" />
              ) : (
                <ChevronRight size={12} className="shrink-0 text-neutral-500" />
              )}
              {iconFor(e.name, true)}
              <span className={`truncate ${e.name.startsWith(".") ? "opacity-60" : ""}`}>{e.name}</span>
            </button>
            {expanded && renderDir(key, depth + 1)}
          </div>
        );
      }
      return (
        <button
          key={key}
          className={`flex w-full items-center gap-1 rounded px-1 py-[3px] text-left text-xs hover:bg-neutral-200/70 dark:hover:bg-neutral-800/70 ${
            selected === key ? "bg-accent-500/10" : ""
          }`}
          style={{ paddingLeft: depth * 12 + 18 }}
          onClick={() => {
            setSelected(key);
            void openFile(key);
          }}
        >
          {iconFor(e.name, false)}
          <span className={`truncate ${e.name.startsWith(".") ? "opacity-60" : ""}`}>{e.name}</span>
        </button>
      );
    });
  };

  const createItem = async (isDir: boolean) => {
    const name = window.prompt(isDir ? "New folder name" : "New file name");
    if (!name) return;
    await api.fsCreate(name, isDir);
    void loadDir("");
  };

  const deleteItem = async () => {
    if (!selected) return;
    if (!await confirmAction(`Delete ${selected}? This cannot be undone.`)) return;
    await api.fsDelete(selected);
    const parent = selected.includes("/") ? selected.slice(0, selected.lastIndexOf("/")) : "";
    void loadDir(parent);
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b border-neutral-200 px-2 py-1.5 dark:border-neutral-800">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-neutral-500">
          Explorer
        </span>
        <div className="flex items-center gap-0.5">
          <IconButton
            title={showHidden ? "Hide hidden files" : "Show hidden files"}
            aria-pressed={showHidden}
            onClick={() => void useApp.getState().updateSettings({ show_hidden_files: !showHidden })}
            className={`h-6 w-6 ${showHidden ? "text-accent-600 dark:text-accent-400" : ""}`}
          >
            {showHidden ? <Eye size={12} /> : <EyeOff size={12} />}
          </IconButton>
          <IconButton title="New file" onClick={() => createItem(false)} className="h-6 w-6">
            <FilePlus size={12} />
          </IconButton>
          <IconButton title="New folder" onClick={() => createItem(true)} className="h-6 w-6">
            <FolderPlus size={12} />
          </IconButton>
          <IconButton title="Delete selected" onClick={deleteItem} className="h-6 w-6">
            <Trash2 size={12} />
          </IconButton>
          <IconButton
            title="Refresh"
            onClick={() => {
              const dirs = ["", ...Array.from(expandedDirs)];
              for (const d of dirs) void loadDir(d);
            }}
            className="h-6 w-6"
          >
            <RefreshCw size={12} />
          </IconButton>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-1">{root ? renderDir("", 0) : null}</div>
    </div>
  );
}
