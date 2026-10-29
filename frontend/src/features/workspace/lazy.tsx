/** Lazily loaded heavy panels (CodeMirror, xterm.js, git) so they stay out of
 *  the initial bundle and load only when a layout actually shows them. */
import { lazy, Suspense, type ReactNode } from "react";
import { Spinner } from "@/components/ui";

export const LazyEditorPane = lazy(() => import("./EditorPane").then((m) => ({ default: m.EditorPane })));
export const LazyTerminalPanel = lazy(() =>
  import("@/features/terminal/TerminalPanel").then((m) => ({ default: m.TerminalPanel })),
);
export const LazyGitPanel = lazy(() => import("@/features/git/GitPanel").then((m) => ({ default: m.GitPanel })));

export function PanelSuspense({ children }: { children: ReactNode }) {
  return (
    <Suspense
      fallback={
        <div className="flex h-full items-center justify-center text-neutral-500">
          <Spinner />
        </div>
      }
    >
      {children}
    </Suspense>
  );
}
