/** Center pane: editor tabs (CodeMirror) + diff tabs. */
import { useMemo, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { javascript } from "@codemirror/lang-javascript";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { go } from "@codemirror/lang-go";
import { java } from "@codemirror/lang-java";
import { cpp } from "@codemirror/lang-cpp";
import { sql } from "@codemirror/lang-sql";
import { php } from "@codemirror/lang-php";
import { xml } from "@codemirror/lang-xml";
import { StreamLanguage } from "@codemirror/language";
import { yaml as yamlMode } from "@codemirror/legacy-modes/mode/yaml";
import { toml as tomlMode } from "@codemirror/legacy-modes/mode/toml";
import { shell as shellMode } from "@codemirror/legacy-modes/mode/shell";
import { dockerFile as dockerMode } from "@codemirror/legacy-modes/mode/dockerfile";
import { ruby as rubyMode } from "@codemirror/legacy-modes/mode/ruby";
import { oneDark } from "@codemirror/theme-one-dark";
import { EditorView } from "@codemirror/view";
import { ChevronDown, X, Circle } from "lucide-react";
import { useEditor, type Tab } from "@/stores/editor";
import { useApp } from "@/stores/app";
import { languageOf } from "@/lib/format";
import { DiffView } from "@/features/diff/DiffView";
import { EmptyState } from "@/components/ui";

function keyOf(t: Tab): string {
  return t.kind === "file" ? `file:${t.path}` : t.key;
}

/** Languages the editor can highlight. Keys mirror `languageOf()` plus the
 *  manual picker below. */
const LANGUAGES: string[] = [
  "typescript", "tsx", "javascript", "jsx", "rust", "python", "json",
  "markdown", "html", "css", "scss", "xml", "go", "java", "cpp", "c",
  "sql", "php", "ruby", "yaml", "toml", "bash", "dockerfile", "text",
];

function langExtension(lang: string): import("@codemirror/state").Extension[] {
  switch (lang) {
    case "typescript":
    case "tsx":
    case "javascript":
    case "jsx":
      return [
        javascript({
          typescript: lang === "typescript" || lang === "tsx",
          jsx: lang === "jsx" || lang === "tsx",
        }),
      ];
    case "rust":
      return [rust()];
    case "python":
      return [python()];
    case "json":
      return [json()];
    case "markdown":
      return [markdown()];
    case "html":
      return [html()];
    case "css":
    case "scss":
      return [css()];
    case "xml":
      return [xml()];
    case "go":
      return [go()];
    case "java":
      return [java()];
    case "cpp":
    case "c":
      return [cpp()];
    case "sql":
      return [sql()];
    case "php":
      return [php()];
    case "ruby":
      return [StreamLanguage.define(rubyMode)];
    case "yaml":
      return [StreamLanguage.define(yamlMode)];
    case "toml":
      return [StreamLanguage.define(tomlMode)];
    case "bash":
      return [StreamLanguage.define(shellMode)];
    case "dockerfile":
      return [StreamLanguage.define(dockerMode)];
    default:
      return [];
  }
}

export function EditorPane() {
  const { tabs, activeKey, setActive, closeTab, update, langOverrides, setLanguage } = useEditor();
  const fontSize = useApp((s) => s.settings.editor_font_size);
  const theme = useApp((s) => s.settings.theme);
  const wordWrap = useApp((s) => s.settings.editor_word_wrap);
  const [langMenu, setLangMenu] = useState(false);
  const active = useMemo(() => tabs.find((t) => keyOf(t) === activeKey) ?? null, [tabs, activeKey]);
  const activeLang =
    active?.kind === "file" ? (langOverrides[active.path] ?? languageOf(active.path)) : null;

  // Height-constrained editor: the scroller MUST opt into overflow explicitly
  // (CodeMirror 6 default styles don't scroll a fixed-height editor).
  const viewTheme = useMemo(
    () =>
      EditorView.theme({
        "&": { fontSize: `${fontSize}px`, height: "100%" },
        ".cm-scroller": {
          fontFamily: "var(--font-mono)",
          height: "100%",
          overflow: "auto",
        },
      }),
    [fontSize],
  );

  const extensions = useMemo(
    () => [
      ...(activeLang ? langExtension(activeLang) : []),
      viewTheme,
      ...(wordWrap ? [EditorView.lineWrapping] : []),
    ],
    [activeLang, viewTheme, wordWrap],
  );

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Tab bar + language picker */}
      <div className="flex h-9 shrink-0 items-stretch border-b border-neutral-200 dark:border-neutral-800">
        <div className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
          {tabs.map((t) => {
            const isActive = keyOf(t) === activeKey;
            const title = t.kind === "file" ? t.path.split("/").pop()! : t.title;
            return (
              <div
                key={keyOf(t)}
                onClick={() => setActive(keyOf(t))}
                className={`group flex cursor-pointer items-center gap-1.5 border-r border-neutral-200 px-3 text-xs dark:border-neutral-800 ${
                  isActive
                    ? "border-b-2 border-b-accent-500 bg-neutral-100 dark:bg-neutral-900"
                    : "hover:bg-neutral-100/70 dark:hover:bg-neutral-800/40"
                }`}
              >
                {t.kind === "file" && t.dirty && <Circle size={7} className="fill-amber-400 text-amber-400" />}
                <span className={isActive ? "font-medium" : "text-neutral-500"}>{title}</span>
                {t.kind === "diff" && (
                  <span className="rounded bg-accent-500/15 px-1 text-[9px] font-semibold text-accent-500">
                    DIFF
                  </span>
                )}
                <button
                  className="ml-1 rounded p-0.5 text-neutral-500 opacity-0 hover:bg-neutral-300/70 group-hover:opacity-100 dark:hover:bg-neutral-700"
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(keyOf(t));
                  }}
                >
                  <X size={11} />
                </button>
              </div>
            );
          })}
        </div>
        {active?.kind === "file" && (
          <div className="relative flex shrink-0 items-center border-l border-neutral-200 pl-1 pr-2 dark:border-neutral-800">
            <button
              title="Editor language (auto-detected from file extension — override if needed)"
              onClick={() => setLangMenu(!langMenu)}
              className="flex items-center gap-1 rounded px-1.5 py-1 text-[10px] font-medium uppercase tracking-wide text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
            >
              {activeLang}
              <ChevronDown size={10} />
            </button>
            {langMenu && (
              <>
                <div className="fixed inset-0 z-30" onClick={() => setLangMenu(false)} />
                <div className="absolute right-1 top-8 z-40 max-h-72 w-40 overflow-y-auto rounded-md border border-neutral-200 bg-white py-1 shadow-xl dark:border-neutral-700 dark:bg-neutral-900">
                  {LANGUAGES.map((l) => (
                    <button
                      key={l}
                      onClick={() => {
                        if (active.kind === "file") setLanguage(active.path, l);
                        setLangMenu(false);
                      }}
                      className={`flex w-full px-3 py-1.5 text-left text-xs hover:bg-neutral-100 dark:hover:bg-neutral-800 ${
                        l === activeLang ? "text-accent-600 dark:text-accent-400" : ""
                      }`}
                    >
                      {l === "text" ? "Plain Text" : l}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* Content — absolutely positioned so the editor box has a guaranteed
          definite height (percentage-height chains proved fragile) and the
          scroller can actually scroll. */}
      <div className="relative min-h-0 flex-1 overflow-hidden">
        {!active && (
          <EmptyState
            title="No file open"
            hint="Select a file in the explorer, or ask the agent to read one."
          />
        )}
        {active?.kind === "file" && (
          <div className="absolute inset-0 overflow-hidden">
            <CodeMirror
              key={active.path}
              value={active.content}
              height="100%"
              className="h-full w-full"
              theme={theme === "dark" ? oneDark : "light"}
              extensions={extensions}
              basicSetup={{
                foldGutter: true,
                highlightActiveLine: true,
                autocompletion: true,
                bracketMatching: true,
                closeBrackets: true,
              }}
              onChange={(value) => update(active.path, value)}
            />
          </div>
        )}
        {active?.kind === "diff" && (
          <div className="absolute inset-0">
            <DiffView diff={active.diff} title={active.title} />
          </div>
        )}
      </div>
    </div>
  );
}
