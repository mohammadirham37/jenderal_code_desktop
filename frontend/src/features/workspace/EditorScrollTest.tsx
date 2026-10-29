/**
 * Standalone scroll/height harness for the real EditorPane — no backend
 * needed. Open the app with `#editor-test` in the URL to render a 300-line
 * file tab and verify that the CodeMirror scroller is bounded and scrollable.
 * Used for regression checks after layout changes.
 */
import { useEffect } from "react";
import { useEditor } from "@/stores/editor";
import { useApp } from "@/stores/app";
import { EditorPane } from "./EditorPane";

export function EditorScrollTest() {
  useEffect(() => {
    const lines = Array.from(
      { length: 300 },
      (_, i) =>
        [
          `// ── baris uji ${i + 1} dari 300 ──`,
          i % 7 === 0
            ? `fn compute_${i}(input: &str) -> Result<String, Error> { Ok(input.repeat(2)) }`
            : i % 7 === 1
              ? `let value_${i} = ${i} * 42 + ${i % 13}; // konstanta`
              : i % 7 === 2
                ? `SELECT id, name FROM users WHERE created_at > now() - interval '7 days';`
                : i % 7 === 3
                  ? `<div className="row-${i}">konten uji scroll editor</div>`
                  : i % 7 === 4
                    ? `{"index": ${i}, "status": "ok", "payload": "abcdefghij"}`
                    : i % 7 === 5
                      ? `# heading ${i}\n\nparagraf panjang untuk menguji scroll vertikal editor.`
                      : `console.log("baris", ${i}); // ${"baris sangat panjang untuk menguji word wrap — ".repeat(4)}`,
        ].join("\n"),
    );
    const content = lines.join("\n");
    useApp.setState({ settings: { ...useApp.getState().settings, theme: "dark" } });
    useEditor.setState({
      tabs: [
        {
          kind: "file",
          path: "scroll-test-sample.txt",
          content,
          original: content,
          dirty: false,
        },
      ],
      activeKey: "file:scroll-test-sample.txt",
      langOverrides: {},
    });
  }, []);

  return (
    <div className="h-full w-full">
      <EditorPane />
    </div>
  );
}
