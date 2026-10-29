/** Command palette (Cmd/Ctrl+K). */
import { useEffect, useMemo, useState } from "react";
import { useApp, type View } from "@/stores/app";
import { useChat } from "@/stores/chat";
import { useEditor } from "@/stores/editor";
import { api } from "@/lib/ipc";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";

interface Command {
  id: string;
  label: string;
  hint?: string;
  run: () => void | Promise<void>;
}

export function CommandPalette() {
  const openState = useApp((s) => s.paletteOpen);
  const setPaletteOpen = useApp((s) => s.setPaletteOpen);
  const setView = useApp((s) => s.setView);
  const openFolderPicker = useApp((s) => s.openFolderPicker);
  const workspace = useApp((s) => s.workspace);
  const updateSettings = useApp((s) => s.updateSettings);
  const settings = useApp((s) => s.settings);
  const layout = useApp((s) => s.layout);
  const setLayout = useApp((s) => s.setLayout);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(!openState);
      }
      if (e.key === "Escape") setPaletteOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openState, setPaletteOpen]);

  const commands = useMemo<Command[]>(() => {
    const go = (v: View, label: string): Command => ({
      id: `go-${v}`,
      label,
      hint: "Go",
      run: () => setView(v),
    });
    const cmds: Command[] = [
      go("welcome", "Go to Projects"),
      go("workspace", "Go to Coding Workspace"),
      go("providers", "Go to Provider Center"),
      go("agents", "Go to Agent Studio"),
      go("sessions", "Go to Sessions"),
      go("settings", "Go to Settings"),
      {
        id: "open-folder",
        label: "Open project folder…",
        hint: "Workspace",
        run: openFolderPicker,
      },
      {
        id: "toggle-theme",
        label: `Switch to ${settings.theme === "dark" ? "light" : "dark"} theme`,
        hint: "Appearance",
        run: () => updateSettings({ theme: settings.theme === "dark" ? "light" : "dark" }),
      },
      {
        id: "save-file",
        label: "Save active file",
        hint: "Editor",
        run: () => useEditor.getState().saveActive(),
      },
    ];
    if (workspace) {
      cmds.push({
        id: "new-session",
        label: "New chat session",
        hint: "Chat",
        run: async () => {
          await useChat.getState().newSession(workspace.id);
          setView("workspace");
        },
      });
      cmds.push({
        id: "toggle-layout",
        label: layout === "chat" ? "Switch to Code layout (editor + chat)" : "Switch to Chat layout (chat-first)",
        hint: "Layout",
        run: () => {
          setLayout(layout === "chat" ? "code" : "chat");
          setView("workspace");
        },
      });
      cmds.push({
        id: "export-history",
        label: "Export history as JSON…",
        hint: "Data",
        run: async () => {
          const target = await saveDialog({
            title: "Export history",
            defaultPath: "jenderalcode-history.json",
            filters: [{ name: "JSON", extensions: ["json"] }],
          });
          if (typeof target === "string") await api.exportHistory(target);
        },
      });
    }
    return cmds;
  }, [setView, openFolderPicker, settings.theme, updateSettings, workspace, layout, setLayout]);

  const filtered = useMemo(() => {
    if (!query.trim()) return commands;
    const q = query.toLowerCase();
    return commands.filter((c) => c.label.toLowerCase().includes(q) || c.hint?.toLowerCase().includes(q));
  }, [commands, query]);

  useEffect(() => setSelected(0), [query, openState]);

  const runCommand = (c: Command) => {
    void c.run();
    setPaletteOpen(false);
    setQuery("");
  };

  if (!openState) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-[15vh]"
      onClick={() => setPaletteOpen(false)}
    >
      <div
        className="w-full max-w-lg overflow-hidden rounded-xl border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-[#15171d] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSelected((i) => Math.min(filtered.length - 1, i + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setSelected((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter" && filtered[selected]) {
              runCommand(filtered[selected]);
            }
          }}
          placeholder="Type a command…"
          className="w-full border-b border-neutral-200 dark:border-neutral-800 bg-transparent px-4 py-3 text-sm focus:outline-none"
        />
        <div className="max-h-72 overflow-y-auto p-1">
          {filtered.map((c, i) => (
            <button
              key={c.id}
              onClick={() => runCommand(c)}
              onMouseEnter={() => setSelected(i)}
              className={`flex w-full items-center justify-between rounded-md px-3 py-2 text-left text-xs ${
                i === selected ? "bg-indigo-500/10 text-indigo-500 dark:text-indigo-300" : ""
              }`}
            >
              <span>{c.label}</span>
              {c.hint && <span className="text-[10px] text-neutral-500">{c.hint}</span>}
            </button>
          ))}
          {filtered.length === 0 && (
            <div className="px-3 py-6 text-center text-xs text-neutral-500">No commands found</div>
          )}
        </div>
      </div>
    </div>
  );
}
