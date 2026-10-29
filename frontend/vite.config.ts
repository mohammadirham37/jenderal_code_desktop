import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Tauri loads the bundle from disk (tauri://localhost), so keep assets relative.
  base: "./",
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "es2022",
    sourcemap: false,
    rollupOptions: {
      output: {
        // Stable vendor chunks: heavy editor/terminal code only loads with the
        // panels that use it, and app updates don't invalidate vendor caches.
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          if (id.includes("@codemirror") || id.includes("@lezer") || id.includes("@uiw")) return "codemirror";
          if (id.includes("@xterm")) return "xterm";
          if (/react-markdown|remark|micromark|mdast|unified|hast|unist|vfile/.test(id)) return "markdown";
          if (/[\\/](react|react-dom|scheduler)[\\/]/.test(id)) return "react";
          return undefined;
        },
      },
    },
  },
});
