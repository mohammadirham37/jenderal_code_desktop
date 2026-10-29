/** Unified/split diff rendering with per-file and per-hunk view. */
import { useMemo, useState } from "react";
import { FileDiff, Plus, Minus } from "lucide-react";
import { parseUnifiedDiff } from "@/lib/format";
import { Button, EmptyState } from "@/components/ui";

export function DiffView({ diff, title }: { diff: string; title: string }) {
  const files = useMemo(() => parseUnifiedDiff(diff), [diff]);
  const [split, setSplit] = useState(false);

  if (!diff.trim()) {
    return <EmptyState icon={<FileDiff size={24} />} title="No changes" />;
  }
  if (files.length === 0) {
    // Not a git-style diff (e.g. a plain preview); show raw.
    return (
      <div className="h-full overflow-auto p-4">
        <div className="mb-2 text-xs font-medium text-neutral-500">{title}</div>
        <pre className="whitespace-pre-wrap font-mono text-xs">{diff}</pre>
      </div>
    );
  }

  const totalAdd = files.reduce((a, f) => a + f.additions, 0);
  const totalDel = files.reduce((a, f) => a + f.deletions, 0);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b border-neutral-200 px-3 py-1.5 dark:border-neutral-800">
        <div className="flex items-center gap-2 text-xs">
          <FileDiff size={13} className="text-indigo-400" />
          <span className="font-medium">{title}</span>
          <span className="text-emerald-500">+{totalAdd}</span>
          <span className="text-red-500">−{totalDel}</span>
        </div>
        <Button size="sm" onClick={() => setSplit(!split)}>
          {split ? "Unified" : "Split"}
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        {files.map((f) => (
          <div key={f.path} className="border-b border-neutral-200 last:border-b-0 dark:border-neutral-800">
            <div className="sticky top-0 z-10 flex items-center gap-2 border-b border-neutral-200 bg-neutral-50 px-3 py-1.5 text-xs dark:border-neutral-800 dark:bg-[#101218]">
              <span className="font-mono">{f.path}</span>
              {f.binary && <span className="text-neutral-500">(binary)</span>}
              <span className="text-emerald-500">+{f.additions}</span>
              <span className="text-red-500">−{f.deletions}</span>
            </div>
            {f.hunks.map((h, i) => (
              <div key={i}>
                <div className="bg-neutral-100 px-3 py-0.5 font-mono text-[10px] text-neutral-500 dark:bg-neutral-800/50">
                  {h.header}
                </div>
                {split ? <SplitHunk lines={h.lines} /> : <UnifiedHunk lines={h.lines} />}
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function UnifiedHunk({ lines }: { lines: { kind: string; text: string; oldNo?: number; newNo?: number }[] }) {
  return (
    <div className="font-mono text-[11px] leading-[1.55]">
      {lines.map((l, i) => (
        <div
          key={i}
          className={`flex ${
            l.kind === "add"
              ? "bg-emerald-500/10"
              : l.kind === "del"
                ? "bg-red-500/10"
                : l.kind === "meta"
                  ? "text-neutral-400"
                  : ""
          }`}
        >
          <LineNo n={l.oldNo} w={46} />
          <LineNo n={l.newNo} w={46} />
          <span className={`w-4 shrink-0 text-center ${signColor(l.kind)}`}>{sign(l.kind)}</span>
          <span className={`whitespace-pre-wrap break-all px-1 ${lineColor(l.kind)}`}>{l.text}</span>
        </div>
      ))}
    </div>
  );
}

function SplitHunk({ lines }: { lines: { kind: string; text: string; oldNo?: number; newNo?: number }[] }) {
  const left = lines.filter((l) => l.kind === "del" || l.kind === "ctx");
  const right = lines.filter((l) => l.kind === "add" || l.kind === "ctx");
  const rows = Math.max(left.length, right.length);
  return (
    <div className="grid grid-cols-2 font-mono text-[11px] leading-[1.55]">
      {Array.from({ length: rows }).map((_, i) => {
        const l = left[i];
        const r = right[i];
        return (
          <div key={i} className="col-span-2 flex">
            <div className={`flex flex-1 ${l?.kind === "del" ? "bg-red-500/10" : ""}`}>
              <LineNo n={l?.oldNo} w={40} />
              <span className="whitespace-pre-wrap break-all px-1 text-red-400/80">{l?.text ?? ""}</span>
            </div>
            <div className={`flex flex-1 ${r?.kind === "add" ? "bg-emerald-500/10" : ""}`}>
              <LineNo n={r?.newNo} w={40} />
              <span className="whitespace-pre-wrap break-all px-1 text-emerald-400/80">{r?.text ?? ""}</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

function LineNo({ n, w }: { n?: number; w: number }) {
  return (
    <span
      className="shrink-0 select-none border-r border-neutral-200/60 pr-1 text-right text-neutral-500 dark:border-neutral-800"
      style={{ width: w }}
    >
      {n ?? ""}
    </span>
  );
}

const sign = (k: string) => (k === "add" ? "+" : k === "del" ? "−" : "");
const signColor = (k: string) =>
  k === "add" ? "text-emerald-500" : k === "del" ? "text-red-500" : "text-neutral-600";
const lineColor = (k: string) =>
  k === "add" ? "text-emerald-300" : k === "del" ? "text-red-300" : "";

export function DiffStat({ additions, deletions }: { additions: number; deletions: number }) {
  return (
    <span className="inline-flex items-center gap-1 text-[10px]">
      <span className="inline-flex items-center gap-0.5 text-emerald-500">
        <Plus size={9} />
        {additions}
      </span>
      <span className="inline-flex items-center gap-0.5 text-red-500">
        <Minus size={9} />
        {deletions}
      </span>
    </span>
  );
}
